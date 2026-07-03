"""MusicOS Python SDK.

A zero-dependency client that drives MusicOS through its MCP server
(``music-server``) over stdio — the exact surface Claude and the CLI use,
so anything they can do, Python can do:

    from musicos import MusicOS

    with MusicOS(project="Song.musicos") as m:
        m.create_project(path="Song.musicos", name="Song")
        m.generate_chords(key="A", scale="minor", bars=8, seed=42)
        m.render_song(output="song.wav")

Any tool from ``m.tools()`` is callable as a method with keyword arguments.
"""

from .client import MusicOS, MusicOSError

__all__ = ["MusicOS", "MusicOSError"]
