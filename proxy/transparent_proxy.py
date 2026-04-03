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
