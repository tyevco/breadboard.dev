/* nrf-saadc (ADR-008 D7, D8; Phase 1 M8): the SAADC as Zephyr's simple
 * mode drives it for E2, bare metal. Every 164 LFCLK ticks (5.005 ms) RTC0
 * wakes the CPU, which
 *   - toggles P0.13 (a plain output, so each burst is visible as an edge);
 *   - converts AIN0 (P0.02): gain 1/6, internal 0.6 V reference, TACQ 40 us,
 *     12 bits, 8x oversampling with BURST: one SAMPLE task runs 8
 *     back-to-back acquisitions in hardware; RESULT.MAXCNT 1, END interrupt;
 *   - converts VDD (PSELP = VDD): gain 1/6, internal reference, TACQ 10 us,
 *     one sample;
 *   - prints "b=<k> ain0=<code> vdd=<code>" on UARTE0 (EasyDMA), one line
 *     per burst, which the Renode engine turns into a log record (D8).
 * The CPU sleeps in WFI between the RTC compare and the SAADC's END
 * interrupt. Board: examples/nrf-saadc/board.cir. */
#include "nrf_gate.h"

#define PIN_MARK 13u
#define PERIOD 164u

static volatile int tick;
static volatile int ended;
static volatile int16_t result[2];
static char line[48];

void RTC0_IRQHandler(void)
{
    if (rtc0_rearm(PERIOD)) {
        tick = 1;
    }
}

void SAADC_IRQHandler(void)
{
    if (NRF_SAADC->EVENTS_END) {
        NRF_SAADC->EVENTS_END = 0;
        ended = 1;
    }
    if (NRF_SAADC->EVENTS_STARTED) {
        NRF_SAADC->EVENTS_STARTED = 0;
    }
}

static int16_t convert(uint32_t ch, volatile int16_t *dst)
{
    NRF_SAADC->RESULT.PTR = (uint32_t)dst;
    NRF_SAADC->RESULT.MAXCNT = 1;
    ended = 0;
    NRF_SAADC->TASKS_START = 1;
    NRF_SAADC->TASKS_SAMPLE = 1;
    while (!ended) {
        __WFI();
    }
    NRF_SAADC->CH[ch].PSELP = SAADC_CH_PSELP_PSELP_NC;
    return *dst;
}

static char *put_str(char *p, const char *s)
{
    while (*s) {
        *p++ = *s++;
    }
    return p;
}

static char *put_int(char *p, int32_t v)
{
    char tmp[12];
    int n = 0;
    if (v < 0) {
        *p++ = '-';
        v = -v;
    }
    do {
        tmp[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v);
    while (n) {
        *p++ = tmp[--n];
    }
    return p;
}

static void uart_write(const char *s, uint32_t n)
{
    NRF_UARTE0->EVENTS_ENDTX = 0;
    NRF_UARTE0->TXD.PTR = (uint32_t)s;
    NRF_UARTE0->TXD.MAXCNT = n;
    NRF_UARTE0->TASKS_STARTTX = 1;
    while (!NRF_UARTE0->EVENTS_ENDTX) {
    }
}

int main(void)
{
    pin_output(PIN_MARK);
    NRF_UARTE0->PSEL.TXD = 6;
    NRF_UARTE0->BAUDRATE = UARTE_BAUDRATE_BAUDRATE_Baud115200;
    NRF_UARTE0->ENABLE = UARTE_ENABLE_ENABLE_Enabled;

    NRF_SAADC->RESOLUTION = SAADC_RESOLUTION_VAL_12bit;
    NRF_SAADC->INTENSET = SAADC_INTENSET_END_Msk;
    NVIC_EnableIRQ(SAADC_IRQn);
    NRF_SAADC->ENABLE = SAADC_ENABLE_ENABLE_Enabled;

    lfclk_start();
    rtc0_start(PERIOD);
    for (int32_t k = 0;; k++) {
        while (!tick) {
            __WFI();
        }
        tick = 0;
        pin_toggle(PIN_MARK);

        NRF_SAADC->OVERSAMPLE = SAADC_OVERSAMPLE_OVERSAMPLE_Over8x;
        NRF_SAADC->CH[0].CONFIG = (SAADC_CH_CONFIG_GAIN_Gain1_6 << SAADC_CH_CONFIG_GAIN_Pos)
                                | (SAADC_CH_CONFIG_REFSEL_Internal << SAADC_CH_CONFIG_REFSEL_Pos)
                                | (SAADC_CH_CONFIG_TACQ_40us << SAADC_CH_CONFIG_TACQ_Pos)
                                | (SAADC_CH_CONFIG_BURST_Enabled << SAADC_CH_CONFIG_BURST_Pos);
        NRF_SAADC->CH[0].PSELP = SAADC_CH_PSELP_PSELP_AnalogInput0;
        int16_t ain0 = convert(0, &result[0]);

        NRF_SAADC->OVERSAMPLE = SAADC_OVERSAMPLE_OVERSAMPLE_Bypass;
        NRF_SAADC->CH[1].CONFIG = (SAADC_CH_CONFIG_GAIN_Gain1_6 << SAADC_CH_CONFIG_GAIN_Pos)
                                | (SAADC_CH_CONFIG_REFSEL_Internal << SAADC_CH_CONFIG_REFSEL_Pos)
                                | (SAADC_CH_CONFIG_TACQ_10us << SAADC_CH_CONFIG_TACQ_Pos);
        NRF_SAADC->CH[1].PSELP = SAADC_CH_PSELP_PSELP_VDD;
        int16_t vdd = convert(1, &result[1]);

        char *p = put_str(line, "b=");
        p = put_int(p, k);
        p = put_str(p, " ain0=");
        p = put_int(p, ain0);
        p = put_str(p, " vdd=");
        p = put_int(p, vdd);
        *p++ = '\n';
        uart_write(line, (uint32_t)(p - line));
    }
}
