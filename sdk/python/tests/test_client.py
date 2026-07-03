"""End-to-end SDK test against a built music-server (stdlib unittest)."""

import os
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from musicos import MusicOS, MusicOSError  # noqa: E402


class ClientTest(unittest.TestCase):
    def test_full_flow(self):
        workdir = tempfile.mkdtemp(prefix="musicos-sdk-")
        bundle = os.path.join(workdir, "Sdk.musicos")
        try:
            with MusicOS() as m:
                tools = m.tools()
                self.assertTrue(any(t["name"] == "render_song" for t in tools))

                m.create_project(path=bundle, name="Sdk")
                out = m.add_track(name="Keys")
                self.assertEqual(out["track_id"], 0)
                chords = m.generate_chords(key="C", scale="major", bars=2, seed=1)
                self.assertEqual(len(chords["progression"]), 2)
                summary = m.get_project_summary()
                self.assertEqual(len(summary["tracks"]), 2)

                with self.assertRaises(MusicOSError):
                    m.remove_track(track_id=99)
                with self.assertRaises(MusicOSError):
                    m.call("no_such_tool")
        finally:
            shutil.rmtree(workdir, ignore_errors=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
