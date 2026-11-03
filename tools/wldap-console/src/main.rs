//! WLDAP Console — a tiny local web app that helps a non-developer set up and
//! verify a WLDAP (wireless CMSIS-DAP) probe without touching a terminal.
//!
//! Serves one page on http://127.0.0.1:8737 and talks to:
//!   * the probe over TCP 4441 (CMSIS-DAP)  — connectivity test
//!   * OpenOCD, started/stopped by this app — child process + log capture
//!   * OpenOCD's GDB port 3333              — polled via netstat, so the UI
//!                                            can show "IDE is attached"
//!
//! No external crates: builds offline with a bare rustup.

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const UI: &str = include_str!("../ui/index.html");

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    /// "exe" (Windows OpenOCD) or "wsl" (OpenOCD inside WSL)
    mode: String,
    /// exe path (mode=exe) or Linux path (mode=wsl); empty means "from PATH"
    openocd: String,
    target: String,
    speed: u32,
    gdb_port: u16,
    /// OpenOCD debug level (-dN). 0 keeps the log readable; 3 is for diagnosis.
    debug: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "192.168.0.102".to_string(),
            port: 4441,
            mode: "wsl".to_string(),
            openocd: String::new(),
            target: "stm32g4x".to_string(),
            speed: 1000,
            gdb_port: 3333,
            debug: 0,
        }
    }
}

impl Config {
    fn load(path: &Path) -> Config {
        let mut cfg = Config::default();
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return cfg,
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "host" => cfg.host = v.trim().to_string(),
                    "port" => cfg.port = v.trim().parse().unwrap_or(cfg.port),
                    "mode" => cfg.mode = v.trim().to_string(),
                    "openocd" => cfg.openocd = v.trim().to_string(),
                    "target" => cfg.target = v.trim().to_string(),
                    "speed" => cfg.speed = v.trim().parse().unwrap_or(cfg.speed),
                    "gdb_port" => cfg.gdb_port = v.trim().parse().unwrap_or(cfg.gdb_port),
                    "debug" => cfg.debug = v.trim().parse().unwrap_or(cfg.debug),
                    _ => {}
                }
            }
        }
        cfg
    }

    fn save(&self, path: &Path) {
        let text = format!(
            "# WLDAP Console settings\nhost={}\nport={}\nmode={}\nopenocd={}\ntarget={}\nspeed={}\ngdb_port={}\ndebug={}\n",
            self.host, self.port, self.mode, self.openocd, self.target, self.speed, self.gdb_port, self.debug
        );
        let _ = fs::write(path, text);
    }
}

struct TestResult {
    ok: bool,
    ms: u64,
    msg: String,
}

/// Windows-side TCP proxy for OpenOCD's GDB port.
///
/// WSL2's localhost relay probes the backend port to decide whether it can
/// forward, and OpenOCD accepts only one GDB client at a time — so the probe
/// steals the slot and the real IDE gets rejected. Owning the listener here
/// avoids the relay entirely and gives an exact "IDE attached" signal.
struct Proxy {
    stop: AtomicBool,
    active: AtomicBool,
    clients: AtomicU32,
}

struct State {
    cfg: Config,
    proxy: Arc<Proxy>,
    child: Option<Child>,
    /// Linux pid of the OpenOCD started inside WSL (mode=wsl).
    wsl_pid: Option<u32>,
    last_alive: Option<Instant>,
    log: VecDeque<String>,
    test: Option<TestResult>,
    started: Option<Instant>,
}

impl State {
    fn new(cfg: Config) -> Self {
        State {
            cfg,
            proxy: Arc::new(Proxy {
                stop: AtomicBool::new(true),
                active: AtomicBool::new(false),
                clients: AtomicU32::new(0),
            }),
            child: None,
            wsl_pid: None,
            last_alive: None,
            log: VecDeque::new(),
            test: None,
            started: None,
        }
    }

    fn push_log(&mut self, line: String) {
        if self.log.len() >= 500 {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    fn running(&self) -> bool {
        self.child.is_some() || self.wsl_pid.is_some()
    }

    fn pid(&self) -> u32 {
        match &self.child {
            Some(c) => c.id(),
            None => self.wsl_pid.unwrap_or(0),
        }
    }

    fn refresh(&mut self) {
        let exited = match self.child.as_mut() {
            Some(ch) => ch.try_wait().ok().flatten(),
            None => None,
        };
        if let Some(status) = exited {
            self.push_log(format!("--- OpenOCD 已退出 ({}) ---", status));
            self.child = None;
            self.started = None;
        }

        // WSL mode: wsl.exe may exit while OpenOCD keeps running, so liveness is
        // tracked by the Linux pid, not by the Windows child.
        if let Some(pid) = self.wsl_pid {
            let due = self
                .last_alive
                .map(|t| t.elapsed() >= Duration::from_secs(2))
                .unwrap_or(true);
            if due {
                self.last_alive = Some(Instant::now());
                if !wsl_alive(pid) {
                    self.push_log("--- OpenOCD 已退出 ---".to_string());
                    self.wsl_pid = None;
                    self.started = None;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let exe_dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let cfg_path = exe_dir.join("config.ini");

    let cfg = Config::load(&cfg_path);
    cfg.save(&cfg_path);

    let state = Arc::new(Mutex::new(State::new(cfg)));

    let mut bound = None;
    for port in 8737..8760u16 {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            bound = Some((l, port));
            break;
        }
    }
    let (listener, port) = match bound {
        Some(v) => v,
        None => {
            eprintln!("无法绑定 127.0.0.1:8737-8759，可能已在运行。");
            std::process::exit(1);
        }
    };

    let url = format!("http://127.0.0.1:{}", port);
    println!("WLDAP Console 已启动: {}", url);
    println!("关闭这个窗口即退出。");
    open_browser(&url);

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let st = Arc::clone(&state);
        let cfgp = cfg_path.clone();
        let dir = exe_dir.clone();
        thread::spawn(move || {
            let _ = handle(stream, st, cfgp, dir);
        });
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn handle(
    mut s: TcpStream,
    state: Arc<Mutex<State>>,
    cfg_path: PathBuf,
    exe_dir: PathBuf,
) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = [0u8; 8192];
    let n = s.read(&mut buf)?;
    if n == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut it = first.split_whitespace();
    let method = it.next().unwrap_or("GET");
    let target = it.next().unwrap_or("/");

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, parse_query(q)),
        None => (target, HashMap::new()),
    };

    let (code, ctype, body) = match (method, path) {
        ("GET", "/") => ("200 OK", "text/html; charset=utf-8", UI.to_string()),

        ("GET", "/api/state") => {
            let mut st = state.lock().unwrap();
            st.refresh();
            sync_log(&mut st, &exe_dir);
            ("200 OK", "application/json", state_json(&st))
        }

        ("POST", "/api/config") => {
            let mut st = state.lock().unwrap();
            if let Some(v) = query.get("host") {
                if !v.trim().is_empty() {
                    st.cfg.host = v.trim().to_string();
                }
            }
            if let Some(v) = query.get("port").and_then(|v| v.parse().ok()) {
                st.cfg.port = v;
            }
            if let Some(v) = query.get("mode") {
                st.cfg.mode = v.clone();
            }
            if let Some(v) = query.get("openocd") {
                st.cfg.openocd = v.trim().to_string();
            }
            if let Some(v) = query.get("target") {
                st.cfg.target = v.clone();
            }
            if let Some(v) = query.get("speed").and_then(|v| v.parse().ok()) {
                st.cfg.speed = v;
            }
            if let Some(v) = query.get("gdb_port").and_then(|v| v.parse().ok()) {
                st.cfg.gdb_port = v;
            }
            st.cfg.save(&cfg_path);
            ("200 OK", "application/json", state_json(&st))
        }

        ("POST", "/api/test") => {
            do_test(&state);
            let mut st = state.lock().unwrap();
            st.refresh();
            ("200 OK", "application/json", state_json(&st))
        }

        ("POST", "/api/detect") => {
            let list = detect_openocd(&exe_dir);
            ("200 OK", "application/json", detect_json(&list))
        }

        ("POST", "/api/start") => {
            if let Some(m) = start_openocd(&state, &exe_dir) {
                let mut st = state.lock().unwrap();
                st.push_log(m);
            }
            let mut st = state.lock().unwrap();
            st.refresh();
            sync_log(&mut st, &exe_dir);
            ("200 OK", "application/json", state_json(&st))
        }

        ("POST", "/api/stop") => {
            stop_proxy(&state);
            let mut st = state.lock().unwrap();
            stop_openocd(&mut st, &exe_dir);
            ("200 OK", "application/json", state_json(&st))
        }

        _ => ("404 Not Found", "text/plain", "not found".to_string()),
    };

    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        code,
        ctype,
        body.as_bytes().len()
    );
    s.write_all(resp.as_bytes())?;
    s.write_all(body.as_bytes())?;
    Ok(())
}

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        map.insert(url_decode(k), url_decode(v));
    }
    map
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---------------------------------------------------------------------------
// State -> JSON
// ---------------------------------------------------------------------------

fn state_json(st: &State) -> String {
    let running = st.running();
    let pid = st.pid();
    let uptime = st.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
    let (mut listening, mut clients) = gdb_status(st.cfg.gdb_port);
    if st.proxy.active.load(Ordering::SeqCst) {
        listening = true;
        clients = st.proxy.clients.load(Ordering::SeqCst) as u32;
    }

    let mut out = String::new();
    out.push('{');
    out.push_str(&format!("\"openocd_running\":{},", running));
    out.push_str(&format!("\"pid\":{},", pid));
    out.push_str(&format!("\"uptime\":{},", uptime));
    out.push_str(&format!("\"host\":{},", json_str(&st.cfg.host)));
    out.push_str(&format!("\"port\":{},", st.cfg.port));
    out.push_str(&format!("\"mode\":{},", json_str(&st.cfg.mode)));
    out.push_str(&format!("\"openocd\":{},", json_str(&st.cfg.openocd)));
    out.push_str(&format!("\"target\":{},", json_str(&st.cfg.target)));
    out.push_str(&format!("\"speed\":{},", st.cfg.speed));
    out.push_str(&format!("\"gdb_port\":{},", st.cfg.gdb_port));
    out.push_str(&format!("\"gdb_listening\":{},", listening));
    out.push_str(&format!("\"gdb_clients\":{},", clients));

    match &st.test {
        Some(t) => out.push_str(&format!(
            "\"test\":{{\"ok\":{},\"ms\":{},\"msg\":{}}},",
            t.ok,
            t.ms,
            json_str(&t.msg)
        )),
        None => out.push_str("\"test\":null,"),
    }

    out.push_str("\"log\":[");
    for (i, l) in st.log.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_str(l));
    }
    out.push_str("]}");
    out
}

fn detect_json(list: &[(String, String)]) -> String {
    let mut out = String::from("[");
    for (i, (path, kind)) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":{},\"kind\":{}}}",
            json_str(path),
            json_str(kind)
        ));
    }
    out.push(']');
    out
}

// ---------------------------------------------------------------------------
// WLDAP connectivity test
// ---------------------------------------------------------------------------

fn do_test(state: &Arc<Mutex<State>>) {
    let (host, port) = {
        let st = state.lock().unwrap();
        (st.cfg.host.clone(), st.cfg.port)
    };
    let result = test_tcp(&host, port);
    let mut st = state.lock().unwrap();
    st.test = Some(result);
}

fn test_tcp(host: &str, port: u16) -> TestResult {
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => {
            return TestResult {
                ok: false,
                ms: 0,
                msg: format!("地址解析失败: {}", e),
            }
        }
    };
    if addrs.is_empty() {
        return TestResult {
            ok: false,
            ms: 0,
            msg: "没有可用地址".to_string(),
        };
    }
    let start = Instant::now();
    match TcpStream::connect_timeout(&addrs[0], Duration::from_millis(2500)) {
        Ok(_) => {
            let ms = start.elapsed().as_millis() as u64;
            TestResult {
                ok: true,
                ms,
                msg: format!("CMSIS-DAP 端口可达 ({} ms)", ms),
            }
        }
        Err(e) => TestResult {
            ok: false,
            ms: start.elapsed().as_millis() as u64,
            msg: format!("连接失败: {}", e),
        },
    }
}

// ---------------------------------------------------------------------------
// OpenOCD discovery
// ---------------------------------------------------------------------------

fn detect_openocd(exe_dir: &Path) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut add = |p: String, k: &str| {
        if !p.trim().is_empty() && !found.iter().any(|(x, _)| *x == p) {
            found.push((p, k.to_string()));
        }
    };

    for rel in [
        "tools/openocd/bin/openocd.exe",
        "openocd/bin/openocd.exe",
        "openocd.exe",
    ] {
        let p = exe_dir.join(rel);
        if p.is_file() {
            add(p.to_string_lossy().to_string(), "exe");
        }
    }

    #[cfg(windows)]
    {
        let mut c = Command::new("where");
        c.arg("openocd");
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        if let Ok(o) = c.output() {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    add(line.trim().to_string(), "exe");
                }
            }
        }

        let mut roots: Vec<PathBuf> = vec![
            PathBuf::from("C:\\Program Files"),
            PathBuf::from("C:\\Program Files (x86)"),
        ];
        if let Ok(up) = env::var("USERPROFILE") {
            roots.push(PathBuf::from(up).join(".espressif").join("tools"));
        }
        for root in roots {
            for exe in find_named(&root, "openocd.exe", 5) {
                add(exe.to_string_lossy().to_string(), "exe");
            }
        }

        let script = "for p in $(command -v openocd 2>/dev/null) \
/home/*/.espressif/tools/openocd-esp32/*/openocd-esp32/bin/openocd \
/usr/bin/openocd /usr/local/bin/openocd; do \
[ -x \"$p\" ] && echo \"$p\" && break; done";
        let mut c = Command::new("wsl.exe");
        c.args(["-e", "sh", "-c", script]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        if let Ok(o) = c.output() {
            if o.status.success() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    let p = line.trim();
                    if p.starts_with('/') {
                        add(p.to_string(), "wsl");
                    }
                }
            }
        }
    }

    found
}

#[cfg(windows)]
fn find_named(root: &Path, name: &str, depth: usize) -> Vec<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let rd = match fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return,
        };
        for e in rd.flatten() {
            let p = e.path();
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                walk(&p, name, depth - 1, out);
            } else if ft.is_file()
                && p.file_name()
                    .map(|n| n.eq_ignore_ascii_case(name))
                    .unwrap_or(false)
            {
                out.push(p);
            }
            if out.len() > 8 {
                return;
            }
        }
    }
    let mut out = Vec::new();
    walk(root, name, depth, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

/// Any child must run from a real Windows directory: when this app is launched
/// from a UNC path, CreateProcess fails with "access denied".
fn sanitize_cwd(cmd: &mut Command) {
    cmd.env_remove("WSL_INTEROP");
    cmd.env_remove("WSL_DISTRO_NAME");
    let dir = if cfg!(windows) {
        env::temp_dir()
    } else {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
    };
    cmd.current_dir(dir);
}

fn gen_cfg(cfg: &Config) -> String {
    // In WSL mode, bind to all interfaces: Windows reaches WSL services through
    // wslrelay, which cannot forward to a 127.0.0.1-only listener.
    let bind = if cfg.mode == "wsl" { "bindto 0.0.0.0\n" } else { "" };
    format!(
        "# Generated by WLDAP Console\n{}\
adapter driver cmsis-dap\n\
cmsis-dap backend tcp\n\
cmsis-dap tcp host {}\n\
cmsis-dap tcp port {}\n\
cmsis-dap tcp min_timeout 500\n\
transport select swd\n\
gdb port {}\n\
# IDE 通常用 Z0（软件断点）请求，但目标代码在 flash 里，OpenOCD 无法真正
# 把 0xBE00 打进 flash，断点静默失效 —— 表现为“停在不了 main”“复位后停不下”.
# 强制用硬件断点：G474 有 6 个比较器，够用。
gdb_breakpoint_override hard\n",
        bind, cfg.host, cfg.port, cfg.gdb_port
    )
}

fn start_openocd(state: &Arc<Mutex<State>>, exe_dir: &Path) -> Option<String> {
    let cfg = {
        let st = state.lock().unwrap();
        if st.running() {
            return Some("OpenOCD 已在运行。".to_string());
        }
        st.cfg.clone()
    };

    // Clear leftovers from a previous run: if the app was closed abruptly the
    // WSL-side OpenOCD survives and would keep the probe (and the port) busy.
    stop_orphans();

    let cfg_file = exe_dir.join("wldap-openocd.cfg");
    if let Err(e) = fs::write(&cfg_file, gen_cfg(&cfg)) {
        return Some(format!("写配置失败: {}", e));
    }
    let target = format!("target/{}.cfg", cfg.target);
    let dbg = if cfg.debug > 0 {
        format!("-d{}", cfg.debug)
    } else {
        String::new()
    };

    if cfg.mode == "wsl" {
        let cfg_in_wsl = match win_to_wsl(&cfg_file) {
            Some(p) => p,
            None => return Some("程序目录不在盘符路径下，WSL 模式解析不了配置。".to_string()),
        };
        let log_file = exe_dir.join("openocd.log");
        let pid_file = exe_dir.join("openocd.pid");
        let (log_in_wsl, pid_in_wsl) = match (win_to_wsl(&log_file), win_to_wsl(&pid_file)) {
            (Some(l), Some(p)) => (l, p),
            _ => return Some("程序目录不在盘符路径下。".to_string()),
        };
        let bin = if cfg.openocd.starts_with('/') {
            cfg.openocd.clone()
        } else {
            "openocd".to_string()
        };
        // $$ becomes OpenOCD's pid via exec, and is written to a file so the
        // Windows side can check liveness even when wsl.exe detaches.
        let script = format!(
            ": > '{}'; echo $$ > '{}'; exec '{}' {} -f '{}' -f '{}' -c 'adapter speed {}' >> '{}' 2>&1",
            log_in_wsl, pid_in_wsl, bin, dbg, cfg_in_wsl, target, cfg.speed, log_in_wsl
        );
        let mut cmd = Command::new("wsl.exe");
        cmd.args(["-e", "sh", "-c", &script]);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.current_dir(exe_dir);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        if let Err(e) = cmd.spawn() {
            return Some(format!("调用 wsl.exe 失败: {}", e));
        }
        thread::sleep(Duration::from_millis(700));

        let pid = fs::read_to_string(&pid_file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok());

        let mut st = state.lock().unwrap();
        match pid {
            Some(pid) => {
                st.push_log(format!(
                    "--- 启动 OpenOCD/WSL (pid {}, 目标 {}, {} kHz{}) ---",
                    pid,
                    cfg.target,
                    cfg.speed,
                    if cfg.debug > 0 { ", 调试日志" } else { "" }
                ));
                st.wsl_pid = Some(pid);
                st.started = Some(Instant::now());
                st.last_alive = Some(Instant::now());
                drop(st);
                if let Err(e) = start_proxy(state, cfg.gdb_port) {
                    state
                        .lock()
                        .unwrap()
                        .push_log(format!("端口转发启动失败: {}", e));
                }
                None
            }
            None => {
                st.push_log("启动失败：拿不到 OpenOCD 的 pid，见日志".to_string());
                Some("启动失败，见日志".to_string())
            }
        }
    } else {
        let bin = if cfg.openocd.trim().is_empty() {
            "openocd".to_string()
        } else {
            cfg.openocd.clone()
        };
        let mut cmd = Command::new(bin);
        if cfg.debug > 0 {
            cmd.arg(format!("-d{}", cfg.debug));
        }
        cmd.arg("-f")
            .arg(&cfg_file)
            .arg("-f")
            .arg(&target)
            .arg("-c")
            .arg(format!("adapter speed {}", cfg.speed));
        cmd.current_dir(exe_dir);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                if let Some(out) = child.stdout.take() {
                    spawn_reader(out, Arc::clone(state));
                }
                if let Some(err) = child.stderr.take() {
                    spawn_reader(err, Arc::clone(state));
                }
                let mut st = state.lock().unwrap();
                st.push_log(format!(
                    "--- 启动 OpenOCD (pid {}, 目标 {}, {} kHz) ---",
                    pid, cfg.target, cfg.speed
                ));
                st.child = Some(child);
                st.started = Some(Instant::now());
                None
            }
            Err(e) => Some(format!("启动 OpenOCD 失败: {}", e)),
        }
    }
}

/// Ask WSL for its VM address (what OpenOCD is reachable on).
fn wsl_ip() -> Option<String> {
    #[cfg(windows)]
    {
        let mut c = Command::new("wsl.exe");
        c.args(["-e", "sh", "-c", "hostname -I"]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        if let Ok(o) = c.output() {
            let out = String::from_utf8_lossy(&o.stdout);
            if let Some(first) = out.split_whitespace().next() {
                return Some(first.to_string());
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Forward 127.0.0.1:<listen> to <wsl_ip>:<listen>.
fn start_proxy(state: &Arc<Mutex<State>>, listen_port: u16) -> Result<(), String> {
    let ip = wsl_ip().ok_or_else(|| "拿不到 WSL 的 IP".to_string())?;
    let proxy = Arc::clone(&state.lock().unwrap().proxy);

    let listener = TcpListener::bind(("127.0.0.1", listen_port)).map_err(|e| {
        format!(
            "端口 {} 被占用（常见于 WSL 残留转发或已有 OpenOCD）：{}。\
             换一个 GDB 端口，或在 PowerShell 执行 wsl --shutdown 后重试。",
            listen_port, e
        )
    })?;
    listener.set_nonblocking(false).ok();

    proxy.stop.store(false, Ordering::SeqCst);
    proxy.clients.store(0, Ordering::SeqCst);
    proxy.active.store(true, Ordering::SeqCst);

    let stop = Arc::clone(&proxy);
    let clients = Arc::clone(&proxy);
    let target = ip.clone();

    thread::spawn(move || {
        for incoming in listener.incoming() {
            if stop.stop.load(Ordering::SeqCst) {
                break;
            }
            let client = match incoming {
                Ok(c) => c,
                Err(_) => continue,
            };
            let clients = Arc::clone(&clients);
            let target = target.clone();
            clients.clients.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
                if let Ok(server) = TcpStream::connect((target.as_str(), listen_port)) {
                    let clones = (
                        client.try_clone(),
                        client.try_clone(),
                        server.try_clone(),
                        server.try_clone(),
                    );
                    if let (Ok(mut c_read), Ok(mut c_write), Ok(mut s_read), Ok(mut s_write)) = clones
                    {
                        // Half-close both sides as soon as either direction ends.
                        // OpenOCD allows a single GDB client, so a leaked
                        // upstream socket would make the next reconnect fail
                        // with "no more connections allowed".
                        let up = thread::spawn(move || {
                            let _ = std::io::copy(&mut c_read, &mut s_write);
                            let _ = s_write.shutdown(Shutdown::Both);
                            let _ = c_read.shutdown(Shutdown::Both);
                        });
                        let _ = std::io::copy(&mut s_read, &mut c_write);
                        let _ = c_write.shutdown(Shutdown::Both);
                        let _ = s_read.shutdown(Shutdown::Both);
                        let _ = up.join();
                    }
                    let _ = server.shutdown(Shutdown::Both);
                }
                clients.clients.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });

    Ok(())
}

fn stop_proxy(state: &Arc<Mutex<State>>) {
    let proxy = Arc::clone(&state.lock().unwrap().proxy);
    if !proxy.active.swap(false, Ordering::SeqCst) {
        return;
    }
    proxy.stop.store(true, Ordering::SeqCst);
    let port = state.lock().unwrap().cfg.gdb_port;
    // Wake the accept loop.
    let _ = TcpStream::connect(("127.0.0.1", port));
}

fn stop_openocd(st: &mut State, exe_dir: &Path) {
    let mut stopped = false;
    if let Some(mut child) = st.child.take() {
        let _ = child.kill();
        let _ = child.wait();
        stopped = true;
    }
    if let Some(pid) = st.wsl_pid.take() {
        let pid_file = exe_dir.join("openocd.pid");
        let rm = win_to_wsl(&pid_file).unwrap_or_default();
        let script = format!("kill -9 {} 2>/dev/null; rm -f '{}' 2>/dev/null; true", pid, rm);
        #[cfg(windows)]
        {
            let mut c = Command::new("wsl.exe");
            c.args(["-e", "sh", "-c", &script]);
            sanitize_cwd(&mut c);
            c.creation_flags(CREATE_NO_WINDOW);
            let _ = c.output();
        }
        #[cfg(not(windows))]
        {
            let _ = script;
        }
        stopped = true;
    }
    stop_orphans();
    st.started = None;
    st.last_alive = None;
    if stopped {
        st.push_log("--- 已停止 OpenOCD ---".to_string());
    }
}

fn wsl_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let mut c = Command::new("wsl.exe");
        c.args(["-e", "sh", "-c", &format!("kill -0 {} 2>/dev/null", pid)]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        return c.output().map(|o| o.status.success()).unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

fn stop_orphans() {
    #[cfg(windows)]
    {
        let mut c = Command::new("wsl.exe");
        c.args(["-e", "pkill", "-f", "wldap-openocd.cfg"]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        let _ = c.output();
    }
}

fn spawn_reader<R: Read + Send + 'static>(r: R, state: Arc<Mutex<State>>) {
    thread::spawn(move || {
        let br = BufReader::new(r);
        for line in br.lines().map_while(Result::ok) {
            if let Ok(mut st) = state.lock() {
                st.push_log(line);
            }
        }
    });
}

/// WSL mode: OpenOCD writes to a file on a Windows drive, so the UI can read
/// it directly instead of pumping pipes across the WSL boundary.
fn sync_log(st: &mut State, exe_dir: &Path) {
    if st.cfg.mode != "wsl" || !st.running() {
        return;
    }
    let text = match fs::read_to_string(exe_dir.join("openocd.log")) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut fresh: VecDeque<String> = VecDeque::new();
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(200);
    for l in &lines[from..] {
        fresh.push_back((*l).to_string());
    }
    st.log = fresh;
}

fn win_to_wsl(p: &Path) -> Option<String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    Some(format!("/mnt/{}{}", drive, &s[2..]))
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", "", url]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        let _ = c.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(url).spawn();
    }
}

// ---------------------------------------------------------------------------
// GDB port observation
// ---------------------------------------------------------------------------

fn gdb_status(port: u16) -> (bool, u32) {
    #[cfg(windows)]
    {
        let mut c = Command::new("netstat");
        c.args(["-ano", "-p", "tcp"]);
        sanitize_cwd(&mut c);
        c.creation_flags(CREATE_NO_WINDOW);
        if let Ok(o) = c.output() {
            let text = String::from_utf8_lossy(&o.stdout);
            let suffix = format!(":{}", port);
            let mut listening = false;
            let mut clients = 0u32;
            for line in text.lines() {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 4 || !cols[1].ends_with(&suffix) {
                    continue;
                }
                if line.contains("LISTENING") {
                    listening = true;
                } else if line.contains("ESTABLISHED") {
                    clients += 1;
                }
            }
            return (listening, clients);
        }
        (false, 0)
    }
    #[cfg(not(windows))]
    {
        let ok = TcpStream::connect(("127.0.0.1", port)).is_ok();
        (ok, 0)
    }
}
