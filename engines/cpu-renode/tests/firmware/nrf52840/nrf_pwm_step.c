/* nrf-pwm-model firmware group (ADR-008 D9, Amendment 9; Phase 1 M10): the
 * PWM, TEMP and NVMC models as E3's homing firmware drives them, bare metal.
 *
 *   1. TEMP: START, sleep in WFI until the DATARDY interrupt, read TEMP.
 *   2. NVMC: write a word at 0xF0000 in write mode (Renode's flash takes it
 *      directly, ADR-008 Amendment 9), then an erase-mode ERASEPAGE of its
 *      page; poll READY, count RTC0 ticks across the erase, read the word
 *      back.
 *   3. PWM0 drives P0.26, P0.05, P0.07, P0.11 (E3's MOT_A_IN1, MOT_A_IN2,
 *      MOT_B_IN1, MOT_B_IN2) as dial-me-spinner docs/firmware.md 5.3 says:
 *      16 MHz, COUNTERTOP 640 (25 kHz), Individual decoder, PWM_REFRESH 6
 *      (446 PPS), two ping-pong sequences of one full step each (8
 *      microsteps x 4 channels), LOOP with the LOOPSDONE_SEQSTART0 short, as
 *      nrfx_pwm_complex_playback sets them. Coil A is cos, coil B sin, in
 *      11.25 degree microsteps; slow decay: for positive current IN1 is held
 *      high and IN2 carries duty 1 - |d| (high = brake), swapped for negative
 *      current; every value has the polarity bit (the period starts high).
 *      The SEQEND interrupt refills the sequence that just loaded its last
 *      value with the next full step. After STEPS full steps one sequence
 *      holds the last microstep, and its SEQEND triggers STOP.
 *   4. One UARTE0 line per part (ADR-008 D8).
 * bb_pwm_test.py computes the same duty table and checks every pin edge.
 * The TEMP and PWM0 handlers go into a RAM copy of startup.c's vector table
 * (VTOR), so startup.c, and with it every other gate image, is unchanged. */
#include "nrf_gate.h"

#define TOP 640u
#define PWM_REFRESH 6u
#define STEPS 4
#define DPK_PERMILLE 880u /* d_peak 0.88: 160 mA x 20.33 Ohm / 3.7 V */
#define FLASH_WORD_ADDR 0xF0000u

static const uint32_t pins[4] = { 26u, 5u, 7u, 11u };
/* cos(11.25 degrees x k) x 10000, k = 0..8 */
static const int32_t cos_tab[9] = { 10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951, 0 };

static uint16_t seq[2][32];
static volatile int temp_ready;
static volatile int stopped;
static volatile uint32_t seqends;
static int next_step; /* the next full step to write into a sequence */
static char line[64];

extern void (*const vector_table[16 + 48])(void);
static void (*ram_vectors[16 + 48])(void) __attribute__((aligned(256)));
void TEMP_IRQHandler(void);
void PWM0_IRQHandler(void);

static void vectors_to_ram(void)
{
    for (int i = 0; i < 16 + 48; i++) {
        ram_vectors[i] = vector_table[i];
    }
    ram_vectors[16 + TEMP_IRQn] = TEMP_IRQHandler;
    ram_vectors[16 + PWM0_IRQn] = PWM0_IRQHandler;
    SCB->VTOR = (uint32_t)ram_vectors;
    __DSB();
}

static int32_t cos_u(int32_t u) /* cos(11.25 u degrees) x 10000 */
{
    u &= 31;
    if (u <= 8) {
        return cos_tab[u];
    }
    if (u <= 16) {
        return -cos_tab[16 - u];
    }
    if (u <= 24) {
        return -cos_tab[u - 16];
    }
    return cos_tab[32 - u];
}

static void coil(uint16_t *dst, int32_t c)
{
    uint32_t mag = (uint32_t)(c < 0 ? -c : c);
    uint32_t amp = (TOP * DPK_PERMILLE / 1000u) * mag / 10000u;
    uint16_t hold = (uint16_t)(0x8000u | TOP);
    uint16_t pwm = (uint16_t)(0x8000u | (TOP - amp));
    if (c >= 0) {
        dst[0] = hold;
        dst[1] = pwm;
    } else {
        dst[0] = pwm;
        dst[1] = hold;
    }
}

/* Full step f (microsteps 8f .. 8f + 7); f >= STEPS holds microstep 8 STEPS - 1. */
static void fill(uint16_t *dst, int f)
{
    for (int j = 0; j < 8; j++) {
        int32_t u = f < STEPS ? 8 * f + j : 8 * STEPS - 1;
        coil(&dst[4 * j], cos_u(u));
        coil(&dst[4 * j + 2], cos_u(u - 8)); /* sin = cos(u - 90 degrees) */
    }
}

void TEMP_IRQHandler(void)
{
    if (NRF_TEMP->EVENTS_DATARDY) {
        NRF_TEMP->EVENTS_DATARDY = 0;
        temp_ready = 1;
    }
}

void PWM0_IRQHandler(void)
{
    for (int n = 0; n < 2; n++) {
        if (NRF_PWM0->EVENTS_SEQEND[n]) {
            NRF_PWM0->EVENTS_SEQEND[n] = 0;
            seqends++;
            if (next_step > STEPS + 1) {
                NRF_PWM0->SHORTS = 0;
                NRF_PWM0->TASKS_STOP = 1;
            } else {
                fill(seq[n], next_step++);
            }
        }
    }
    if (NRF_PWM0->EVENTS_STOPPED) {
        NRF_PWM0->EVENTS_STOPPED = 0;
        NRF_PWM0->ENABLE = 0;
        stopped = 1;
    }
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

static char *put_hex(char *p, uint32_t v)
{
    for (int k = 7; k >= 0; k--) {
        *p++ = "0123456789abcdef"[(v >> (4 * k)) & 0xF];
    }
    return p;
}

static void uart_line(char *end)
{
    *end++ = '\n';
    NRF_UARTE0->EVENTS_ENDTX = 0;
    NRF_UARTE0->TXD.PTR = (uint32_t)line;
    NRF_UARTE0->TXD.MAXCNT = (uint32_t)(end - line);
    NRF_UARTE0->TASKS_STARTTX = 1;
    while (!NRF_UARTE0->EVENTS_ENDTX) {
    }
}

int main(void)
{
    vectors_to_ram();
    NRF_UARTE0->PSEL.TXD = 6;
    NRF_UARTE0->BAUDRATE = UARTE_BAUDRATE_BAUDRATE_Baud115200;
    NRF_UARTE0->ENABLE = UARTE_ENABLE_ENABLE_Enabled;
    lfclk_start();
    NRF_RTC0->PRESCALER = 0;
    NRF_RTC0->TASKS_START = 1;

    /* 1. TEMP */
    NRF_TEMP->INTENSET = TEMP_INTENSET_DATARDY_Msk;
    NVIC_EnableIRQ(TEMP_IRQn);
    NRF_TEMP->TASKS_START = 1;
    while (!temp_ready) {
        __WFI();
    }
    int32_t t = NRF_TEMP->TEMP;
    NRF_TEMP->TASKS_STOP = 1;
    char *p = put_str(line, "temp=");
    p = put_int(p, t);
    uart_line(p);

    /* 2. NVMC page erase */
    volatile uint32_t *w = (volatile uint32_t *)FLASH_WORD_ADDR;
    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Wen << NVMC_CONFIG_WEN_Pos;
    *w = 0xC0FFEE00u;
    while (NRF_NVMC->READY == NVMC_READY_READY_Busy) {
    }
    uint32_t before = *w;
    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Een << NVMC_CONFIG_WEN_Pos;
    uint32_t r0 = NRF_RTC0->COUNTER;
    NRF_NVMC->ERASEPAGE = FLASH_WORD_ADDR;
    uint32_t polls = 0;
    while (NRF_NVMC->READY == NVMC_READY_READY_Busy) {
        polls++;
    }
    uint32_t r1 = NRF_RTC0->COUNTER;
    NRF_NVMC->CONFIG = NVMC_CONFIG_WEN_Ren << NVMC_CONFIG_WEN_Pos;
    p = put_str(line, "nvmc before=");
    p = put_hex(p, before);
    p = put_str(p, " after=");
    p = put_hex(p, *w);
    p = put_str(p, " ticks=");
    p = put_int(p, (int32_t)((r1 - r0) & 0xFFFFFFu));
    p = put_str(p, " polled=");
    p = put_int(p, polls > 0);
    uart_line(p);

    /* 3. PWM0 */
    for (int c = 0; c < 4; c++) {
        pin_output(pins[c]);
        NRF_PWM0->PSEL.OUT[c] = pins[c];
    }
    fill(seq[0], 0);
    fill(seq[1], 1);
    next_step = 2;
    NRF_PWM0->ENABLE = PWM_ENABLE_ENABLE_Enabled;
    NRF_PWM0->MODE = PWM_MODE_UPDOWN_Up;
    NRF_PWM0->PRESCALER = PWM_PRESCALER_PRESCALER_DIV_1;
    NRF_PWM0->COUNTERTOP = TOP;
    NRF_PWM0->DECODER = (PWM_DECODER_LOAD_Individual << PWM_DECODER_LOAD_Pos)
                      | (PWM_DECODER_MODE_RefreshCount << PWM_DECODER_MODE_Pos);
    for (int n = 0; n < 2; n++) {
        NRF_PWM0->SEQ[n].PTR = (uint32_t)seq[n];
        NRF_PWM0->SEQ[n].CNT = 32;
        NRF_PWM0->SEQ[n].REFRESH = PWM_REFRESH;
        NRF_PWM0->SEQ[n].ENDDELAY = 0;
    }
    NRF_PWM0->LOOP = 0xFFFF;
    NRF_PWM0->SHORTS = PWM_SHORTS_LOOPSDONE_SEQSTART0_Msk;
    NRF_PWM0->INTEN = PWM_INTEN_SEQEND0_Msk | PWM_INTEN_SEQEND1_Msk | PWM_INTEN_STOPPED_Msk;
    NVIC_EnableIRQ(PWM0_IRQn);
    NRF_PWM0->TASKS_SEQSTART[0] = 1;
    while (!stopped) {
        __WFI();
    }
    p = put_str(line, "pwm seqends=");
    p = put_int(p, (int32_t)seqends);
    p = put_str(p, " stopped");
    uart_line(p);
    for (;;) {
        __WFI();
    }
}
