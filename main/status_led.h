/*
 * SPDX-FileCopyrightText: 2026 WLDAP contributors
 * SPDX-License-Identifier: Apache-2.0
 *
 * Board status LED for WLDAP.
 *
 * This is separate from the per-interface CMSIS-DAP LED (CONFIG_ESP_DAP_n_LED_*),
 * which only follows the DAP_HostStatus "debugger connected" command. The
 * status LED reports what the *board* is doing: boot, WiFi, client connected,
 * or SWD traffic in progress.
 */

#pragma once

#include "sdkconfig.h"

typedef enum {
    /* Nothing to report; LED stays dark. */
    STATUS_LED_OFF = 0,
    /* Slow 1 Hz pulse: associating with the AP. */
    STATUS_LED_WIFI_CONNECTING,
    /* Three quick blips then a pause: association failed. */
    STATUS_LED_WIFI_FAILED,
    /* Short blip every 2 s: WiFi up, no debugger attached. */
    STATUS_LED_READY,
    /* Solid on: a TCP client (OpenOCD/IAR/pyOCD) is attached. */
    STATUS_LED_CONNECTED,
    /* Fast triple blip: fatal error, about to reboot. */
    STATUS_LED_ERROR,
    STATUS_LED_COUNT,
} status_led_state_t;

#if defined(CONFIG_WLDAP_STATUS_LED)

/* Configure the GPIO and leave the LED dark. */
void status_led_init(void);

/* Blocking power-on signature (a few short blinks). Call before starting
 * the WiFi stack so the user sees the board came alive. */
void status_led_boot_pattern(void);

/* Create the pattern task. Call once, after status_led_init(). */
void status_led_start(void);

/* Select the semantic state. Safe to call from any task, ISR-unsafe. */
void status_led_set(status_led_state_t state);

/* Report one CMSIS-DAP request. Sustained traffic is shown as activity;
 * sustained *continuous* traffic (flash download) gets a distinct pattern. */
void status_led_note_dap_activity(void);

#else /* !CONFIG_WLDAP_STATUS_LED */

static inline void status_led_init(void) {}
static inline void status_led_boot_pattern(void) {}
static inline void status_led_start(void) {}
static inline void status_led_set(status_led_state_t state) { (void)state; }
static inline void status_led_note_dap_activity(void) {}

#endif /* CONFIG_WLDAP_STATUS_LED */
