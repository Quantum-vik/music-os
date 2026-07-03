"""MCP-over-stdio client for MusicOS (stdlib only)."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from typing import Any


class MusicOSError(RuntimeError):
    """A tool call or protocol error reported by MusicOS."""


class MusicOS:
    """A MusicOS session over the ``music-server`` MCP stdio transport.

    Args:
        project: Path to the ``.musicos`` bundle the session is scoped to.
            Optional — ``create_project`` can create and activate one.
        server_bin: Path to ``music-server``. Defaults to the
            ``MUSICOS_SERVER`` env var, then ``music-server`` on PATH.
    """

    def __init__(self, project: str | None = None, server_bin: str | None = None):
        binary = server_bin or os.environ.get("MUSICOS_SERVER") or shutil.which("music-server")
        if not binary:
            raise MusicOSError(
                "music-server not found - build it (cargo build -p musicos-server) "
                "and set MUSICOS_SERVER or put it on PATH"
            )
        args = [binary]
        if project:
            args += ["--project", project]
        self._proc = subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        self._next_id = 0
        self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "musicos-python", "version": "0.1.0"},
            },
        )
        self._notify("notifications/initialized")

    # -- protocol ---------------------------------------------------------

    def _send(self, message: dict[str, Any]) -> None:
        assert self._proc.stdin is not None
        self._proc.stdin.write(json.dumps(message) + "\n")
        self._proc.stdin.flush()

    def _notify(self, method: str) -> None:
        self._send({"jsonrpc": "2.0", "method": method})

    def _request(self, method: str, params: dict[str, Any] | None = None) -> Any:
        self._next_id += 1
        rid = self._next_id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})
        assert self._proc.stdout is not None
        for line in self._proc.stdout:
            response = json.loads(line)
            if response.get("id") != rid:
                continue
            if "error" in response:
                raise MusicOSError(response["error"].get("message", "protocol error"))
            return response["result"]
        raise MusicOSError("music-server exited unexpectedly")

    # -- public API -------------------------------------------------------

    def tools(self) -> list[dict[str, Any]]:
        """Every available tool (name, description, inputSchema)."""
        return self._request("tools/list")["tools"]

    def call(self, tool: str, **arguments: Any) -> dict[str, Any]:
        """Calls a tool by name; raises :class:`MusicOSError` on failure."""
        result = self._request("tools/call", {"name": tool, "arguments": arguments})
        text = result["content"][0]["text"]
        if result.get("isError"):
            raise MusicOSError(text)
        # Tool outputs are "summary\n{json}"; return the structured part.
        lines = text.split("\n", 1)
        payload = lines[1] if len(lines) == 2 else lines[0]
        try:
            return json.loads(payload)
        except json.JSONDecodeError:
            return {"summary": text}

    def close(self) -> None:
        """Terminates the server subprocess."""
        if self._proc.poll() is None:
            assert self._proc.stdin is not None
            self._proc.stdin.close()
            self._proc.wait(timeout=5)

    def __enter__(self) -> "MusicOS":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def __getattr__(self, name: str):
        """Unknown attributes become tool calls: ``m.add_track(name="X")``."""
        if name.startswith("_"):
            raise AttributeError(name)

        def tool_method(**arguments: Any) -> dict[str, Any]:
            return self.call(name, **arguments)

        return tool_method
