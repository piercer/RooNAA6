"""
Capture proxy: sits between HQPlayer and T8 to record exact binary framing.
Run this INSTEAD of naa_proxy.py when you want to capture what HQPlayer sends.

Usage:
  1. Kill the normal proxy: fuser -k 43210/tcp 43210/udp
  2. Start this: python3 capture_proxy.py
  3. In HQPlayer web UI, refresh devices, select "Capture Proxy"
  4. Play a track FROM HQPLAYER'S OWN LIBRARY (not Roon) that has cover art
  5. Let it play ~10 seconds, then stop
  6. Chunks saved to /tmp/naa_capture/

The proxy is fully transparent — it doesn't modify any data.
"""
import socket, threading, struct, os, datetime, sys

T8_HOST = "192.168.30.109"
NAA_PORT = 43210
CAPTURE_DIR = "/tmp/naa_capture"
MCAST_ADDRS = ["224.0.0.199", "239.192.0.199"]
IFACE_IP = "192.168.30.212"

DISCOVER_RESPONSE = (
    '<?xml version="1.0" encoding="utf-8"?>'
    '<networkaudio>'
    '<discover result="OK" name="Capture Proxy" version="eversolo naa" protocol="6" trigger="0">'
    'network audio'
    '</discover>'
    '</networkaudio>\n'
).encode("utf-8")

chunk_counter = 0

def ts():
    return datetime.datetime.now().strftime("%H:%M:%S.%f")[:-3]

def save_chunk(direction, data):
    global chunk_counter
    chunk_counter += 1
    fname = os.path.join(CAPTURE_DIR, f"{chunk_counter:06d}_{direction}.bin")
    with open(fname, 'wb') as f:
        f.write(data)
    # Log interesting content
    extra = ""
    if b'[metadata]' in data:
        extra = " **METADATA**"
    if b'\xff\xd8' in data:
        extra += " **JPEG_START**"
    if b'\xff\xd9' in data:
        extra += " **JPEG_END**"
    if data.lstrip().startswith(b'<'):
        try:
            text = data.decode('utf-8').strip()
            if 'keepalive' not in text:
                extra += f" XML: {text[:200]}"
        except:
            pass
    size = len(data)
    # Show first 32 bytes as hex
    hexdump = data[:32].hex()
    print(f"{ts()} [{direction}] #{chunk_counter} {size}b {hexdump}{extra}", flush=True)

def discovery_responder():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", NAA_PORT))
    for addr in MCAST_ADDRS:
        mreq = struct.pack("4s4s", socket.inet_aton(addr), socket.inet_aton(IFACE_IP))
        try:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
        except OSError:
            pass
    print(f"{ts()} [discovery] listening on :{NAA_PORT} (mcast on {IFACE_IP})", flush=True)
    while True:
        try:
            data, addr = sock.recvfrom(4096)
            if b"discover" in data and b"network audio" in data:
                sock.sendto(DISCOVER_RESPONSE, addr)
                print(f"{ts()} [discovery] responded to {addr}", flush=True)
        except OSError:
            pass

def forward(src, dst, direction):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [{direction}] EOF", flush=True)
                break
            save_chunk(direction, data)
            dst.sendall(data)
    except OSError as e:
        print(f"{ts()} [{direction}] error: {e}", flush=True)

if __name__ == '__main__':
    os.makedirs(CAPTURE_DIR, exist_ok=True)
    # Clear old captures
    for f in os.listdir(CAPTURE_DIR):
        os.remove(os.path.join(CAPTURE_DIR, f))
    print(f"{ts()} Capture dir: {CAPTURE_DIR} (cleared)", flush=True)

    threading.Thread(target=discovery_responder, daemon=True).start()

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", NAA_PORT))
    listener.listen(5)
    print(f"{ts()} Capture proxy: :{NAA_PORT} -> {T8_HOST}:{NAA_PORT}", flush=True)
    print(f"{ts()} Select 'Capture Proxy' in HQPlayer, play a track WITH COVER ART from HQPlayer library", flush=True)

    while True:
        client, addr = listener.accept()
        print(f"{ts()} HQP connected from {addr}", flush=True)
        try:
            t8 = socket.create_connection((T8_HOST, NAA_PORT), timeout=5)
        except Exception as e:
            print(f"{ts()} T8 connect failed: {e}", flush=True)
            client.close()
            continue
        t1 = threading.Thread(target=forward, args=(client, t8, "HQP_T8"), daemon=True)
        t2 = threading.Thread(target=forward, args=(t8, client, "T8_HQP"), daemon=True)
        t1.start(); t2.start()
        t1.join(); t2.join()
        client.close(); t8.close()
        print(f"{ts()} Session ended — {chunk_counter} chunks captured", flush=True)
