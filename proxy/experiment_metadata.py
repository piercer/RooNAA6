"""
Metadata injection experiment.

Transparent TCP proxy like transparent_proxy.py, but after PlaylistAdd is
forwarded it tries injecting several candidate XML messages to HQPlayer to
see which (if any) change the title shown on the T8.

Watch stdout — any HQPlayer response to the injected messages will appear
as [HQP→Roon] lines. Check the T8 display after each injection attempt.

Usage:
    python experiment_metadata.py
"""

import socket
import threading
import datetime
import time

HQPLAYER_HOST = "192.168.30.212"
HQPLAYER_PORT = 4321
LISTEN_PORT   = 4321

# Candidate messages to try. Each will be sent 2 seconds apart after PlaylistAdd.
# Watch the T8 and terminal after each one.
CANDIDATES = [
    '<?xml version="1.0" encoding="utf-8"?><SetSong title="Hello World" artist="Test Artist" album="Test Album"/>\n',
    '<?xml version="1.0" encoding="utf-8"?><SetMetadata title="Hello World" artist="Test Artist" album="Test Album"/>\n',
    '<?xml version="1.0" encoding="utf-8"?><Song title="Hello World" artist="Test Artist" album="Test Album"/>\n',
    '<?xml version="1.0" encoding="utf-8"?><Metadata title="Hello World" artist="Test Artist" album="Test Album"/>\n',
    '<?xml version="1.0" encoding="utf-8"?><SetTitle title="Hello World" artist="Test Artist" album="Test Album"/>\n',
]


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


def forward_roon_to_hqp(
    roon_sock: socket.socket,
    hqp_sock: socket.socket,
    hqp_lock: threading.Lock,
    playlist_added: threading.Event,
) -> None:
    """Forward Roon→HQP, set playlist_added event when PlaylistAdd is seen."""
    src_file = roon_sock.makefile("rb")
    try:
        for line in src_file:
            log("Roon→HQP", line)
            if b"PlaylistAdd" in line and b"secure_uri" in line:
                playlist_added.set()
            try:
                with hqp_lock:
                    hqp_sock.sendall(line)
            except OSError:
                break
    except OSError:
        pass
    finally:
        src_file.close()


def injector(
    hqp_sock: socket.socket,
    hqp_lock: threading.Lock,
    playlist_added: threading.Event,
) -> None:
    """Wait for PlaylistAdd, then try each candidate message 2s apart."""
    print("Injector: waiting for PlaylistAdd...", flush=True)
    if not playlist_added.wait(timeout=60):
        print("Injector: timed out waiting for PlaylistAdd", flush=True)
        return

    # Give HQPlayer a moment to finish the PlaylistAdd / Play exchange
    time.sleep(1.5)

    for i, msg in enumerate(CANDIDATES, 1):
        print(f"\nInjector [{i}/{len(CANDIDATES)}]: sending →\n  {msg.strip()}", flush=True)
        print("  >>> Watch the T8 display now <<<", flush=True)
        try:
            with hqp_lock:
                hqp_sock.sendall(msg.encode("utf-8"))
        except OSError as e:
            print(f"Injector: send failed: {e}", flush=True)
            return
        time.sleep(3)

    print("\nInjector: all candidates sent. Check T8 and logs above.", flush=True)


def main() -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", LISTEN_PORT))
    listener.listen(1)
    print(f"Metadata experiment proxy on 0.0.0.0:{LISTEN_PORT} → {HQPLAYER_HOST}:{HQPLAYER_PORT}", flush=True)
    print(f"Will try {len(CANDIDATES)} candidate metadata messages after PlaylistAdd.", flush=True)

    while True:
        print("\nWaiting for Roon connection...", flush=True)
        roon_sock, roon_addr = listener.accept()
        print(f"Roon connected from {roon_addr}", flush=True)

        try:
            hqp_sock = socket.create_connection((HQPLAYER_HOST, HQPLAYER_PORT), timeout=10)
        except OSError as e:
            print(f"Failed to connect to HQPlayer: {e}", flush=True)
            roon_sock.close()
            continue

        print(f"Connected to HQPlayer at {HQPLAYER_HOST}:{HQPLAYER_PORT}", flush=True)

        hqp_lock = threading.Lock()
        playlist_added = threading.Event()

        t_roon = threading.Thread(
            target=forward_roon_to_hqp,
            args=(roon_sock, hqp_sock, hqp_lock, playlist_added),
            daemon=True,
        )
        t_hqp = threading.Thread(
            target=forward,
            args=(hqp_sock, roon_sock, "HQP→Roon"),
            daemon=True,
        )
        t_inject = threading.Thread(
            target=injector,
            args=(hqp_sock, hqp_lock, playlist_added),
            daemon=True,
        )

        t_roon.start()
        t_hqp.start()
        t_inject.start()

        t_roon.join()
        t_hqp.join()

        roon_sock.close()
        hqp_sock.close()
        print("Session ended.", flush=True)


if __name__ == "__main__":
    main()
