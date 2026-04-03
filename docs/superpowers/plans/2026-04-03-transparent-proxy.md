# Transparent TCP Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a minimal logging transparent TCP proxy that forwards all bytes between Roon and HQPlayer on port 4321, printing each XML line to stdout, to validate that Roon's `secure_uri` works through a proxy.

**Architecture:** Single Python script with a `forward()` function that reads line-by-line from one socket, logs to stdout, and writes to the other. Two threads run `forward()` in opposite directions. The main loop accepts one Roon connection at a time, opens a matching outbound connection to HQPlayer, runs the threads, then loops.

**Tech Stack:** Python 3 stdlib only — `socket`, `threading`, `datetime`. No dependencies to install.

---

### Task 1: Scaffold proxy directory and write the forwarding function with tests

**Files:**
- Create: `proxy/transparent_proxy.py`
- Create: `proxy/test_transparent_proxy.py`

- [ ] **Step 1: Create the proxy directory and empty script**

```bash
mkdir -p proxy
touch proxy/transparent_proxy.py
touch proxy/test_transparent_proxy.py
```

- [ ] **Step 2: Write the failing tests for `forward()`**

Open `proxy/test_transparent_proxy.py` and write:

```python
import socket
import threading
import time
from transparent_proxy import forward

def socket_pair():
    """Return a connected (server_sock, client_sock) pair on loopback."""
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]
    client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    client.connect(("127.0.0.1", port))
    server, _ = srv.accept()
    srv.close()
    return server, client


def test_forward_passes_line_through(capsys):
    """Lines read from src arrive unchanged at dst."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "A→B"), daemon=True)
    t.start()

    src_client.sendall(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n")
    dst_client.settimeout(2)
    received = dst_client.recv(1024)
    assert received == b"<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n"

    src_client.close()
    t.join(timeout=2)


def test_forward_logs_xml_to_stdout(capsys):
    """Each forwarded line is printed to stdout with direction label."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "Roon→HQP"), daemon=True)
    t.start()

    src_client.sendall(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><Stop/>\n")
    time.sleep(0.1)

    captured = capsys.readouterr()
    assert "[Roon→HQP]" in captured.out
    assert "<Stop/>" in captured.out

    src_client.close()
    t.join(timeout=2)


def test_forward_exits_on_src_close():
    """forward() returns when the source socket closes."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "A→B"), daemon=True)
    t.start()

    src_client.close()
    t.join(timeout=2)
    assert not t.is_alive(), "forward() should have returned after src closed"
```

- [ ] **Step 3: Run tests to confirm they fail**

```bash
cd proxy && python -m pytest test_transparent_proxy.py -v
```

Expected: `ImportError` or `ModuleNotFoundError` — `transparent_proxy` doesn't exist yet.

- [ ] **Step 4: Write the minimal `forward()` implementation**

Open `proxy/transparent_proxy.py` and write:

```python
import socket
import threading
import datetime

HQPLAYER_HOST = "192.168.30.212"
HQPLAYER_PORT = 4321
LISTEN_PORT   = 4321


def log(label: str, line: bytes) -> None:
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    try:
        text = line.decode("utf-8", errors="replace").rstrip("\n")
    except Exception:
        text = repr(line)
    print(f"{ts} [{label}] {text}", flush=True)


def forward(src: socket.socket, dst: socket.socket, label: str) -> None:
    """Read lines from src, log them, write to dst. Returns when src closes."""
    src_file = src.makefile("rb")
    try:
        for line in src_file:
            log(label, line)
            try:
                dst.sendall(line)
            except OSError:
                break
    except OSError:
        pass
    finally:
        src_file.close()
```

- [ ] **Step 5: Run tests to confirm they pass**

```bash
cd proxy && python -m pytest test_transparent_proxy.py -v
```

Expected output:
```
test_transparent_proxy.py::test_forward_passes_line_through PASSED
test_transparent_proxy.py::test_forward_logs_xml_to_stdout PASSED
test_transparent_proxy.py::test_forward_exits_on_src_close PASSED
3 passed
```

- [ ] **Step 6: Commit**

```bash
cd ..
git add proxy/transparent_proxy.py proxy/test_transparent_proxy.py
git commit -m "feat: add transparent proxy with tested forward() function"
```

---

### Task 2: Add the main proxy loop

**Files:**
- Modify: `proxy/transparent_proxy.py` — append `main()` and `__main__` block

- [ ] **Step 1: Append `main()` to `proxy/transparent_proxy.py`**

Add this to the end of the file:

```python
def main() -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", LISTEN_PORT))
    listener.listen(1)
    print(f"Transparent proxy listening on 0.0.0.0:{LISTEN_PORT} → {HQPLAYER_HOST}:{HQPLAYER_PORT}", flush=True)

    while True:
        print("Waiting for Roon connection...", flush=True)
        roon_sock, roon_addr = listener.accept()
        print(f"Roon connected from {roon_addr}", flush=True)

        try:
            hqp_sock = socket.create_connection((HQPLAYER_HOST, HQPLAYER_PORT), timeout=10)
        except OSError as e:
            print(f"Failed to connect to HQPlayer at {HQPLAYER_HOST}:{HQPLAYER_PORT}: {e}", flush=True)
            roon_sock.close()
            continue

        print(f"Connected to HQPlayer at {HQPLAYER_HOST}:{HQPLAYER_PORT}", flush=True)

        t1 = threading.Thread(target=forward, args=(roon_sock, hqp_sock, "Roon→HQP"), daemon=True)
        t2 = threading.Thread(target=forward, args=(hqp_sock, roon_sock, "HQP→Roon"), daemon=True)
        t1.start()
        t2.start()

        t1.join()
        t2.join()

        roon_sock.close()
        hqp_sock.close()
        print("Session ended.", flush=True)


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Verify existing tests still pass**

```bash
cd proxy && python -m pytest test_transparent_proxy.py -v
```

Expected: `3 passed`

- [ ] **Step 3: Smoke-test that the script starts up**

```bash
cd proxy && timeout 2 python transparent_proxy.py || true
```

Expected: prints `Transparent proxy listening on 0.0.0.0:4321 → 192.168.30.212:4321` then exits after 2 seconds (killed by timeout). If you see `Address already in use`, port 4321 is taken — check with `ss -tlnp | grep 4321`.

- [ ] **Step 4: Commit**

```bash
cd ..
git add proxy/transparent_proxy.py
git commit -m "feat: add main proxy loop — ready for live test"
```

---

### Task 3: Live validation test

This task is manual. No code changes — just run the proxy and point Roon at it.

- [ ] **Step 1: Find this machine's IP on the HiFi network**

```bash
ip route get 192.168.30.212
```

Look for `src <IP>` in the output — that is the IP Roon needs to reach.

- [ ] **Step 2: Start the proxy**

```bash
cd proxy && python transparent_proxy.py
```

Leave this terminal open. You should see: `Waiting for Roon connection...`

- [ ] **Step 3: Reconfigure Roon to point at this machine**

In Roon → Settings → Audio → HQPlayer:
- Change the HQPlayer address from `192.168.30.212` to the IP from Step 1
- Save

- [ ] **Step 4: Play a track in Roon**

Select the HQPlayer output and play something.

**Success looks like:** Audio plays on the T8 AND the terminal shows XML lines flowing both ways, e.g.:
```
2026-04-03 10:14:01 [Roon→HQP] <?xml version="1.0" encoding="utf-8"?><GetInfo/>
2026-04-03 10:14:01 [HQP→Roon] <?xml version="1.0" encoding="utf-8"?><GetInfo engine="5.35.6" .../>
2026-04-03 10:14:01 [Roon→HQP] <?xml version="1.0" encoding="utf-8"?><SessionAuthentication .../>
...
2026-04-03 10:14:02 [Roon→HQP] <?xml version="1.0" encoding="utf-8"?><PlaylistAdd secure_uri="roon://..." .../>
```

**Failure looks like:** Roon shows an error, no audio, or the proxy terminal shows no messages after Roon connects.

- [ ] **Step 5: Record the result in CLAUDE.md**

Update the "Current state" section of `CLAUDE.md` to record what happened (works / fails / partial), and paste a few representative log lines if it works.

- [ ] **Step 6: Commit the updated CLAUDE.md**

```bash
git add CLAUDE.md
git commit -m "docs: record transparent proxy validation result"
```
