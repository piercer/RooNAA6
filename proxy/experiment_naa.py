"""
NAA transparent proxy / traffic capture experiment.

Sits between HQPlayer and the T8 NAA renderer. Logs all binary traffic
as hex+ASCII so we can find the metadata fields in the NAA protocol.

To use:
  1. Start this script
  2. In HQPlayer, change the NAA renderer IP from 192.168.30.109 to this
     machine's IP (keep the same port)
  3. Play a track — the T8 should still play audio
  4. Inspect the log for title/artist/album strings

Usage:
    python experiment_naa.py [--port PORT]

Defaults: listen on 0.0.0.0:NAA_PORT, forward to T8_HOST:NAA_PORT
"""

import socket
import threading
import datetime
import sys
import argparse

T8_HOST    = "192.168.30.109"
NAA_PORT   = 4444          # standard NAA port — change if HQPlayer shows different
LISTEN_PORT = NAA_PORT

CHUNK = 4096               # bytes to read per recv()


def ts() -> str:
    return datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]


def hexdump(label: str, data: bytes) -> None:
    """Print a labelled hex+ASCII dump of data."""
    if not data:
        return
    lines = []
    for i in range(0, len(data), 16):
        chunk = data[i:i+16]
        hex_part  = " ".join(f"{b:02x}" for b in chunk)
        ascii_part = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
        lines.append(f"  {i:04x}  {hex_part:<47}  {ascii_part}")
    block = "\n".join(lines)
    print(f"{ts()} [{label}] {len(data)} bytes\n{block}", flush=True)


def scan_strings(label: str, data: bytes, min_len: int = 4) -> None:
    """Print any printable ASCII strings found in data (for quick metadata spotting)."""
    current = []
    found = []
    for b in data:
        if 32 <= b < 127:
            current.append(chr(b))
        else:
            if len(current) >= min_len:
                found.append("".join(current))
            current = []
    if len(current) >= min_len:
        found.append("".join(current))
    if found:
        print(f"{ts()} [{label}] strings: {found}", flush=True)


def forward(src: socket.socket, dst: socket.socket, label: str) -> None:
    """Forward raw bytes src→dst, logging everything."""
    try:
        while True:
            data = src.recv(CHUNK)
            if not data:
                break
            hexdump(label, data)
            scan_strings(label, data)
            try:
                dst.sendall(data)
            except OSError:
                break
    except OSError:
        pass


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=NAA_PORT,
                        help=f"NAA port (default {NAA_PORT})")
    args = parser.parse_args()

    listen_port = args.port

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", listen_port))
    listener.listen(1)
    print(f"NAA capture proxy: 0.0.0.0:{listen_port} → {T8_HOST}:{listen_port}", flush=True)
    print(f"Reconfigure HQPlayer's NAA output IP from {T8_HOST} to this machine's IP.", flush=True)
    print(f"All traffic will be logged below as hex+ASCII.\n", flush=True)

    while True:
        print("Waiting for HQPlayer connection...", flush=True)
        hqp_sock, hqp_addr = listener.accept()
        print(f"HQPlayer connected from {hqp_addr}", flush=True)

        try:
            t8_sock = socket.create_connection((T8_HOST, listen_port), timeout=10)
        except OSError as e:
            print(f"Failed to connect to T8 at {T8_HOST}:{listen_port}: {e}", flush=True)
            hqp_sock.close()
            continue

        print(f"Connected to T8 at {T8_HOST}:{listen_port}\n", flush=True)

        t1 = threading.Thread(target=forward, args=(hqp_sock, t8_sock, "HQP→T8"),  daemon=True)
        t2 = threading.Thread(target=forward, args=(t8_sock, hqp_sock, "T8→HQP"),  daemon=True)
        t1.start()
        t2.start()
        t1.join()
        t2.join()

        hqp_sock.close()
        t8_sock.close()
        print("\nSession ended.", flush=True)


if __name__ == "__main__":
    main()
