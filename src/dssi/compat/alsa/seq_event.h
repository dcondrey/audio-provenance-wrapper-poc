// Minimal stand-in for <alsa/seq_event.h>, used only where ALSA headers are
// absent (macOS, CI without libasound2-dev). It covers what dssi.h and the
// DSSI shim touch and reproduces the ALSA sequencer ABI layout (28 bytes on
// LP64) so a DSSI host built against real ALSA passes compatible events.
// Linux builds use the system header when CMake finds it.
#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned char snd_seq_event_type_t;

enum
{
    SND_SEQ_EVENT_NOTE = 5,
    SND_SEQ_EVENT_NOTEON = 6,
    SND_SEQ_EVENT_NOTEOFF = 7,
    SND_SEQ_EVENT_KEYPRESS = 8,
    SND_SEQ_EVENT_CONTROLLER = 10,
    SND_SEQ_EVENT_PGMCHANGE = 11,
    SND_SEQ_EVENT_CHANPRESS = 12,
    SND_SEQ_EVENT_PITCHBEND = 13
};

typedef struct snd_seq_addr
{
    unsigned char client;
    unsigned char port;
} snd_seq_addr_t;

typedef struct snd_seq_ev_note
{
    unsigned char channel;
    unsigned char note;
    unsigned char velocity;
    unsigned char off_velocity;
    unsigned int duration;
} snd_seq_ev_note_t;

typedef struct snd_seq_ev_ctrl
{
    unsigned char channel;
    unsigned char unused[3];
    unsigned int param;
    signed int value;
} snd_seq_ev_ctrl_t;

typedef unsigned int snd_seq_tick_time_t;

typedef struct snd_seq_real_time
{
    unsigned int tv_sec;
    unsigned int tv_nsec;
} snd_seq_real_time_t;

typedef union snd_seq_timestamp
{
    snd_seq_tick_time_t tick;
    struct snd_seq_real_time time;
} snd_seq_timestamp_t;

typedef struct snd_seq_event
{
    snd_seq_event_type_t type;
    unsigned char flags;
    signed char tag;
    unsigned char queue;
    snd_seq_timestamp_t time;
    snd_seq_addr_t source;
    snd_seq_addr_t dest;
    union
    {
        snd_seq_ev_note_t note;
        snd_seq_ev_ctrl_t control;
        unsigned char raw8[12];
        unsigned int raw32[3];
    } data;
} snd_seq_event_t;

#ifdef __cplusplus
static_assert (sizeof (snd_seq_event_t) == 28, "snd_seq_event_t must match the ALSA layout");
}
#endif
