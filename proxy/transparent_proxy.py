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
