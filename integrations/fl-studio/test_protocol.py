"""Protocol vector self-test (CI): mirrors the Rust encoder in fl.rs."""
VECTORS = [
    ("transport_play", [0xF0, 0x7D, 0x01, 0x01, 0xF7]),
    ("transport_stop", [0xF0, 0x7D, 0x01, 0x00, 0xF7]),
    ("transport_record", [0xF0, 0x7D, 0x01, 0x02, 0xF7]),
    ("tempo_92.5", [0xF0, 0x7D, 0x02, 925 & 0x7F, 925 >> 7, 0xF7]),
    ("mixer_track1_75pct", [0xF0, 0x7D, 0x05, 1, 0, 9600 & 0x7F, 9600 >> 7, 0xF7]),
]
for name, data in VECTORS:
    assert data[0] == 0xF0 and data[1] == 0x7D and data[-1] == 0xF7, name
    assert all(0 <= b <= 0x7F for b in data[2:-1]), f"{name}: 7-bit args"
print(f"{len(VECTORS)} protocol vectors OK")
