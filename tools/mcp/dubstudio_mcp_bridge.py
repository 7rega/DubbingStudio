#!/usr/bin/env python3
"""
DubStudio MCP Bridge: stdio JSON-RPC 2.0 to Streamable HTTP.
Connects any standard MCP client (Claude Code, Antigravity, Cursor, OpenCode, Codex)
to DubStudio's native MCP server with automatic dynamic port discovery.
"""

import sys
import os
import json
import urllib.request
import urllib.error

# Ensure UTF-8 mode on Windows
if sys.platform == "win32":
    try:
        sys.stdin.reconfigure(encoding="utf-8", errors="replace")
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

LOG_FILE = os.path.join(os.path.dirname(__file__), "dubstudio_mcp.log")
CURRENT_URL = None


def log(msg):
    try:
        with open(LOG_FILE, "a", encoding="utf-8") as f:
            f.write(msg + "\n")
    except Exception:
        pass


def test_mcp_url(url, timeout=0.3):
    try:
        status_url = url.rstrip("/")
        if status_url.endswith("/mcp"):
            status_url = f"{status_url}/status"
        req = urllib.request.Request(status_url)
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("server_name") == "dubstudio":
                return True
    except Exception:
        pass
    return False


def discover_mcp_url():
    # 1. Environment variable override
    env_url = os.environ.get("DUB_STUDIO_URL")
    if env_url:
        return env_url

    # 2. Known port files
    temp_dir = os.environ.get("TEMP", os.environ.get("TMP", "/tmp"))
    candidates = [
        os.path.join(temp_dir, "dubstudio.port"),
        os.path.join(os.path.dirname(__file__), ".port"),
        r"F:\DubStudio\workspace\.port",
        r"F:\DubbingStudio\workspace\.port",
    ]
    for p in candidates:
        if os.path.isfile(p):
            try:
                with open(p, "r", encoding="utf-8") as f:
                    val = f.read().strip()
                    if val.isdigit():
                        cand_url = f"http://127.0.0.1:{val}/mcp"
                        if test_mcp_url(cand_url, timeout=0.2):
                            return cand_url
                    elif val.startswith("http"):
                        if test_mcp_url(val, timeout=0.2):
                            return val
            except Exception:
                pass

    # 3. Default port 8765
    default_url = "http://127.0.0.1:8765/mcp"
    if test_mcp_url(default_url, timeout=0.2):
        return default_url

    # 4. Scan listening ports on localhost
    try:
        import subprocess
        cmd = ["powershell", "-NoProfile", "-Command", "Get-NetTCPConnection -State Listen -LocalAddress 127.0.0.1 | Select-Object -ExpandProperty LocalPort"]
        out = subprocess.check_output(cmd, timeout=3, text=True, stderr=subprocess.DEVNULL)
        ports = [int(line.strip()) for line in out.splitlines() if line.strip().isdigit()]
        for port in ports:
            if port == 8765:
                continue
            cand_url = f"http://127.0.0.1:{port}/mcp"
            if test_mcp_url(cand_url, timeout=0.15):
                return cand_url
    except Exception:
        pass

    return default_url


def get_url():
    global CURRENT_URL
    if CURRENT_URL and test_mcp_url(CURRENT_URL, timeout=0.2):
        return CURRENT_URL
    CURRENT_URL = discover_mcp_url()
    return CURRENT_URL


def send_to_mcp(body_bytes):
    url = get_url()
    req = urllib.request.Request(
        url,
        data=body_bytes,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.read()
    except (urllib.error.URLError, ConnectionError):
        # Refresh URL and retry once (e.g. if DubStudio was restarted on a new port)
        global CURRENT_URL
        CURRENT_URL = discover_mcp_url()
        req = urllib.request.Request(
            CURRENT_URL,
            data=body_bytes,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=120) as resp:
            return resp.read()


def read_message():
    buffer_bytes = b""
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            if buffer_bytes.strip():
                return buffer_bytes.strip(), False
            return None, False

        stripped = line.strip().lower()
        if not buffer_bytes and stripped.startswith(b"content-length:"):
            try:
                length = int(stripped.split(b":")[1].strip())
                while True:
                    hdr = sys.stdin.buffer.readline()
                    if hdr.strip() == b"":
                        break
                body = sys.stdin.buffer.read(length)
                return body, True
            except Exception as e:
                log(f"Error reading header framing: {e}")
                continue

        buffer_bytes += line
        stripped_buf = buffer_bytes.strip()
        if not stripped_buf:
            buffer_bytes = b""
            continue

        if stripped_buf.startswith(b"{") or stripped_buf.startswith(b"["):
            try:
                json.loads(stripped_buf.decode("utf-8"))
                return stripped_buf, False
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue


def main():
    url = get_url()
    log(f"=== DubStudio MCP Bridge Started -> {url} ===")
    while True:
        msg_bytes, is_header_framed = read_message()
        if msg_bytes is None:
            log("EOF on stdin, exiting.")
            break

        msg_id = None
        try:
            req_obj = json.loads(msg_bytes.decode("utf-8", errors="replace"))
            msg_id = req_obj.get("id")
            method = req_obj.get("method", "")
            log(f">> REQ [{method}]: {str(req_obj)[:180]}")
        except Exception:
            pass

        try:
            resp_bytes = send_to_mcp(msg_bytes)
            if msg_id is None or not resp_bytes or resp_bytes.strip() == b"":
                # Notifications do not have responses in JSON-RPC stdio
                continue

            try:
                resp_obj = json.loads(resp_bytes.decode("utf-8"))
                resp_compact_bytes = json.dumps(resp_obj, ensure_ascii=False).encode("utf-8")
            except Exception:
                resp_compact_bytes = resp_bytes.strip()

            if is_header_framed:
                hdr = f"Content-Length: {len(resp_compact_bytes)}\r\n\r\n".encode("ascii")
                sys.stdout.buffer.write(hdr + resp_compact_bytes)
            else:
                sys.stdout.buffer.write(resp_compact_bytes + b"\n")
            sys.stdout.buffer.flush()

        except Exception as e:
            target = CURRENT_URL or url
            log(f"Connection error to {target}: {e}")
            err_payload = {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {
                    "code": -32000,
                    "message": f"DubStudio is not reachable at {target}. Please start Dub Studio and make sure the server is running."
                }
            }
            err_bytes = json.dumps(err_payload, ensure_ascii=False).encode("utf-8")
            if is_header_framed:
                hdr = f"Content-Length: {len(err_bytes)}\r\n\r\n".encode("ascii")
                sys.stdout.buffer.write(hdr + err_bytes)
            else:
                sys.stdout.buffer.write(err_bytes + b"\n")
            sys.stdout.buffer.flush()


if __name__ == "__main__":
    main()
