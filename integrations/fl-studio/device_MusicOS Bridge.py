# name=MusicOS Bridge
# url=https://github.com/Quantum-vik/music-os
"""MusicOS -> FL Studio bridge (MIDI Controller Script).

Install: copy this folder to
  Documents/Image-Line/FL Studio/Settings/Hardware/MusicOS Bridge/
then in FL: Options > MIDI settings > select the "MusicOS Bridge" input
port and set Controller type to "MusicOS Bridge".

Protocol: SysEx F0 7D <cmd> <7-bit args...> F7 (see fl.rs for the encoder).
Notes streamed on regular MIDI channels pass through to the selected
channel; while recording they land in the piano roll.
"""

import midi
import transport
import mixer
import channels
import patterns
import plugins
import general

CMD_TRANSPORT = 0x01   # arg0: 0=stop 1=play 2=record-toggle
CMD_TEMPO = 0x02       # arg0..1: bpm*10 as 14-bit (lo7, hi7)
CMD_SELECT_PATTERN = 0x03  # arg0..1: pattern number, 14-bit
CMD_SELECT_CHANNEL = 0x04  # arg0..1: channel index, 14-bit
CMD_MIXER_LEVEL = 0x05     # arg0..1: track 14-bit; arg2..3: level 14-bit of 0..12800 (0..1.0*12800)
CMD_PLUGIN_PARAM = 0x06    # arg0..1: channel; arg2..3: param; arg4..5: value 14-bit of 0..16383
CMD_METRONOME = 0x07       # arg0: 0=off 1=on


def _u14(lo, hi):
    return (hi << 7) | lo


def OnInit():
    print("MusicOS Bridge ready")


def handle(cmd, a):
    if cmd == CMD_TRANSPORT:
        if a[0] == 0:
            transport.stop()
        elif a[0] == 1:
            transport.start()
        elif a[0] == 2:
            transport.record()
    elif cmd == CMD_TEMPO:
        # processRECEvent on the tempo REC id expects bpm*1000
        bpm10 = _u14(a[0], a[1])
        general.processRECEvent(
            midi.REC_Tempo, bpm10 * 100, midi.REC_Control | midi.REC_UpdateControl
        )
    elif cmd == CMD_SELECT_PATTERN:
        patterns.jumpToPattern(_u14(a[0], a[1]))
    elif cmd == CMD_SELECT_CHANNEL:
        channels.selectOneChannel(_u14(a[0], a[1]))
    elif cmd == CMD_MIXER_LEVEL:
        track = _u14(a[0], a[1])
        level = _u14(a[2], a[3]) / 12800.0
        mixer.setTrackVolume(track, min(level, 1.0))
    elif cmd == CMD_PLUGIN_PARAM:
        channel = _u14(a[0], a[1])
        param = _u14(a[2], a[3])
        value = _u14(a[4], a[5]) / 16383.0
        plugins.setParamValue(value, param, channel)
    elif cmd == CMD_METRONOME:
        if transport.isMetronomeEnabled() != bool(a[0]):
            transport.globalTransport(midi.FPT_Metronome, 1)


def OnSysEx(event):
    data = list(event.sysex)
    # F0 7D cmd ... F7
    if len(data) >= 4 and data[0] == 0xF0 and data[1] == 0x7D and data[-1] == 0xF7:
        handle(data[2], data[3:-1])
        event.handled = True
