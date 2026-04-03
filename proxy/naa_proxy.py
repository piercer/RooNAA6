import socket, threading, datetime, struct

T8_HOST = "192.168.30.109"
NAA_PORT = 43210

DISCOVER_RESPONSE = (
    '<?xml version="1.0" encoding="utf-8"?>'
    '<networkaudio>'
    '<discover result="OK" name="RooNAA6 Proxy" version="eversolo naa" protocol="6" trigger="0">'
    'network audio'
    '</discover>'
    '</networkaudio>\n'
).encode("utf-8")

MCAST_ADDRS = ["224.0.0.199", "239.192.0.199"]

def ts():
    return datetime.datetime.now().strftime("%H:%M:%S.%f")[:-3]

def log_xml(label, data):
    stripped = data.lstrip()
    if stripped.startswith(b'<') and b'keepalive' not in data:
        try:
            text = data.decode('utf-8', errors='replace').rstrip()
            print(f"{ts()} [{label}] {text}", flush=True)
        except Exception:
            pass

def forward(src, dst, label):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [{label}] EOF", flush=True)
                break
            log_xml(label, data)
            dst.sendall(data)
    except OSError as e:
        print(f"{ts()} [{label}] error: {e}", flush=True)

def discovery_responder():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", NAA_PORT))
    for addr in MCAST_ADDRS:
        mreq = struct.pack("4sL", socket.inet_aton(addr), socket.INADDR_ANY)
        try:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
        except OSError:
            pass
    while True:
        try:
            data, addr = sock.recvfrom(4096)
            if b"discover" in data and b"network audio" in data:
                sock.sendto(DISCOVER_RESPONSE, addr)
        except OSError:
            pass

threading.Thread(target=discovery_responder, daemon=True).start()

listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("0.0.0.0", NAA_PORT))
listener.listen(5)
print(f"{ts()} NAA proxy ready: :43210 -> {T8_HOST}:43210", flush=True)

while True:
    client, addr = listener.accept()
    print(f"{ts()} HQP connected from {addr}", flush=True)
    try:
        t8 = socket.create_connection((T8_HOST, NAA_PORT), timeout=5)
    except Exception as e:
        print(f"{ts()} T8 connect failed: {e}", flush=True)
        client.close()
        continue
    t1 = threading.Thread(target=forward, args=(client, t8, "HQP->T8"), daemon=True)
    t2 = threading.Thread(target=forward, args=(t8, client, "T8->HQP"), daemon=True)
    t1.start(); t2.start()
    t1.join(); t2.join()
    client.close(); t8.close()
    print(f"{ts()} Session ended", flush=True)
