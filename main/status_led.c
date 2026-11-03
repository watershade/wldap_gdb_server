/*
 * SPDX-FileCopyrightText: 2026 WLDAP contributors
 * SPDX-License-Identifier: Apache-2.0
 *
 * Board status LED for WLDAP.
 *
 * Patterns are time-based, driven from one low-priority task, so nothing here
 * blocks the DAP path. The LED pin is expected to be wired as a plain GPIO
 * (no WS2812), with the polarity selected in Kconfig. On the ESP32-C5-MINI
 * prototype the blue LED is on GPIO23 and is active LOW.
 */

#include "sdkconfig.h"

#if defined(CONFIG_WLDAP_STATUS_LED)

#include <stdbool.h>
#include <stdint.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "driver/gpio.h"
#include "esp_timer.h"

#include "status_led.h"

#define LED_PIN         CONFIG_WLDAP_STATUS_LED_GPIO
#define TASK_TICK_MS    10

/* A quiet gap longer than this ends the current activity burst. */
#define ACTIVITY_HOLD_US    (300 * 1000)
/* Continuous traffic for at least this long is treated as a bulk transfer
 * (flash erase/program), not interactive single stepping. */
#define BULK_RUN_US         (800 * 1000)

struct led_step {
    uint16_t on_ms;
    uint16_t off_ms;
};

enum led_shape {
    LED_SHAPE_OFF,
    LED_SHAPE_ON,
    LED_SHAPE_BLINK,
};

struct led_pattern {
    enum led_shape shape;
    const struct led_step *steps;
    uint8_t count;
};

/* Boot: four 120 ms blinks, then off (synchronous, see below). */
#define BOOT_BLINKS     4
#define BOOT_ON_MS      120
#define BOOT_OFF_MS     120

/* WiFi association in progress: 1 Hz, mostly dark. */
static const struct led_step wifi_connecting[] = {
    { 120, 880 },
};

/* Association failed: three quick blips, then a long pause. */
static const struct led_step wifi_failed[] = {
    { 80, 80 }, { 80, 80 }, { 80, 700 },
};

/* Idle and reachable: one short blip every 2 s. */
static const struct led_step ready[] = {
    { 40, 1960 },
};

/* DAP traffic: ~17 Hz twinkle, clearly "busy" but not solid. */
static const struct led_step activity[] = {
    { 25, 45 },
};

/* Bulk transfer: effectively solid, with a brief dip every 500 ms so it is
 * still distinguishable from the solid "client connected" state. */
static const struct led_step bulk[] = {
    { 500, 20 },
};

/* Fatal error: six fast blips, then a pause, repeating until reboot. */
static const struct led_step error[] = {
    { 60, 60 }, { 60, 60 }, { 60, 60 }, { 60, 1100 },
};

static const struct led_pattern patterns[STATUS_LED_COUNT] = {
    [STATUS_LED_OFF]             = { LED_SHAPE_OFF,   NULL,            0 },
    [STATUS_LED_WIFI_CONNECTING] = { LED_SHAPE_BLINK, wifi_connecting, 1 },
    [STATUS_LED_WIFI_FAILED]     = { LED_SHAPE_BLINK, wifi_failed,     3 },
    [STATUS_LED_READY]           = { LED_SHAPE_BLINK, ready,           1 },
    [STATUS_LED_CONNECTED]       = { LED_SHAPE_ON,    NULL,            0 },
    [STATUS_LED_ERROR]           = { LED_SHAPE_BLINK, error,           4 },
};

static const struct led_pattern pattern_activity = {
    LED_SHAPE_BLINK, activity, 1
};
static const struct led_pattern pattern_bulk = {
    LED_SHAPE_BLINK, bulk, 1
};

static volatile status_led_state_t s_state = STATUS_LED_OFF;
static volatile int64_t s_last_activity_us;
static volatile int64_t s_activity_start_us;

static inline void led_write(bool on)
{
#if defined(CONFIG_WLDAP_STATUS_LED_ACTIVE_HIGH)
    gpio_set_level(LED_PIN, on ? 1 : 0);
#else
    gpio_set_level(LED_PIN, on ? 0 : 1);
#endif
}

void status_led_init(void)
{
    gpio_config_t io = {
        .pin_bit_mask = 1ULL << LED_PIN,
        .mode = GPIO_MODE_OUTPUT,
        .pull_up_en = GPIO_PULLUP_DISABLE,
        .pull_down_en = GPIO_PULLDOWN_DISABLE,
        .intr_type = GPIO_INTR_DISABLE,
    };
    gpio_config(&io);

    led_write(false);   /* Dark until the boot pattern starts. */
}

void status_led_boot_pattern(void)
{
    for (int i = 0; i < BOOT_BLINKS; i++) {
        led_write(true);
        vTaskDelay(pdMS_TO_TICKS(BOOT_ON_MS));
        led_write(false);
        vTaskDelay(pdMS_TO_TICKS(BOOT_OFF_MS));
    }
}

void status_led_set(status_led_state_t state)
{
    if (state >= STATUS_LED_COUNT)
        state = STATUS_LED_OFF;
    s_state = state;
}

void status_led_note_dap_activity(void)
{
    int64_t now = esp_timer_get_time();

    /* Start a new burst if the previous one has already gone quiet. */
    if (now - s_last_activity_us > ACTIVITY_HOLD_US)
        s_activity_start_us = now;
    s_last_activity_us = now;
}

/* Pick the pattern that should be showing right now: the semantic state,
 * overlaid with the activity indication while a client is attached. */
static const struct led_pattern *current_pattern(int64_t now)
{
    status_led_state_t state = s_state;

    if (state == STATUS_LED_READY || state == STATUS_LED_CONNECTED) {
        if (now - s_last_activity_us < ACTIVITY_HOLD_US) {
            if (now - s_activity_start_us >= BULK_RUN_US)
                return &pattern_bulk;
            return &pattern_activity;
        }
    }

    return &patterns[state];
}

static void status_led_task(void *arg)
{
    (void)arg;

    const struct led_pattern *pat = NULL;
    uint8_t idx = 0;
    bool on_phase = true;
    int64_t phase_start = esp_timer_get_time();

    while (true) {
        int64_t now = esp_timer_get_time();
        const struct led_pattern *want = current_pattern(now);

        if (want != pat) {
            pat = want;
            idx = 0;
            on_phase = true;
            phase_start = now;
            led_write(pat->shape != LED_SHAPE_OFF);
            vTaskDelay(pdMS_TO_TICKS(TASK_TICK_MS));
            continue;
        }

        if (pat->shape != LED_SHAPE_BLINK) {
            vTaskDelay(pdMS_TO_TICKS(TASK_TICK_MS));
            continue;
        }

        const struct led_step *step = &pat->steps[idx];
        int64_t duration_us = (int64_t)(on_phase ? step->on_ms : step->off_ms) * 1000;

        if (now - phase_start < duration_us) {
            vTaskDelay(pdMS_TO_TICKS(TASK_TICK_MS));
            continue;
        }

        phase_start = now;
        on_phase = !on_phase;
        led_write(on_phase);

        if (on_phase) {
            /* The off phase just finished: advance to the next step, wrapping. */
            idx++;
            if (idx >= pat->count)
                idx = 0;
        }
    }
}

void status_led_start(void)
{
    xTaskCreate(status_led_task, "status_led", 2048, NULL, 1, NULL);
}

#endif /* CONFIG_WLDAP_STATUS_LED */
