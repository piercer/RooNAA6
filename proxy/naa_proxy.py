import socket, threading, datetime, struct, os, json

T8_HOST = "192.168.30.109"
NAA_PORT = 43210
METADATA_FILE = "/tmp/roon_now_playing.json"

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
    if not stripped.startswith(b'<') or b'keepalive' in data:
        return
    try:
        text = data.decode('utf-8')
        if all(32 <= ord(c) < 127 or c in '\n\r\t' for c in text[:50]):
            print(f"{ts()} [{label}] {text.rstrip()}", flush=True)
    except (UnicodeDecodeError, ValueError):
        pass

def get_roon_metadata():
    """Read current track metadata from Roon metadata listener."""
    try:
        with open(METADATA_FILE, 'r') as f:
            return json.load(f)
    except:
        return {}

def replace_metadata_section(data, jpeg_data=None):
    """Replace [metadata] section content with Roon metadata, keeping exact same byte count.

    If jpeg_data is provided, splice it immediately after the metadata null terminator.
    When jpeg_data is None, byte count is preserved (backward-compatible behavior).
    """
    marker = b'\x00[metadata]\n'
    mpos = data.find(marker)
    if mpos == -1:
        return data, False

    section_start = mpos + len(marker)
    section_end = data.find(b'\x00', section_start)
    if section_end == -1:
        return data, False

    target_len = section_end - section_start

    # Get real Roon metadata
    meta = get_roon_metadata()
    title = meta.get("title", "")
    artist = meta.get("artist", "")
    album = meta.get("album", "")

    if not title:
        return data, False

    # Build metadata-only content (no audio format fields — T8 knows them from start)
    # Prioritize fitting song > artist > album within target_len
    new_content = f'song={title}\nartist={artist}\nalbum={album}\n'.encode('utf-8')

    if len(new_content) > target_len:
        # Drop album to fit
        new_content = f'song={title}\nartist={artist}\n'.encode('utf-8')
    if len(new_content) > target_len:
        # Truncate title
        max_title = target_len - len(f'song=\nartist={artist}\n')
        new_content = f'song={title[:max_title]}\nartist={artist}\n'.encode('utf-8')

    # Pad to exact target length
    if len(new_content) < target_len:
        padding = target_len - len(new_content)
        new_content = new_content[:-1] + b' ' * padding + b'\n'
    elif len(new_content) > target_len:
        new_content = new_content[:target_len - 1] + b'\n'

    before_meta = data[:section_start]
    after_null = data[section_end + 1:]  # everything after the metadata null

    if jpeg_data:
        modified = before_meta + new_content + b'\x00' + jpeg_data + after_null
    else:
        modified = before_meta + new_content + b'\x00' + after_null
    return modified, True

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

def forward_hqp_to_t8(src, dst):
    started = False
    injected_count = 0
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [HQP->T8] EOF", flush=True)
                break
            log_xml("HQP->T8", data)

            if b'type="start"' in data and b'result=' not in data:
                started = True
                meta = get_roon_metadata()
                print(f"{ts()} [HQP->T8] start detected — Roon: {meta.get('artist', '?')} - {meta.get('title', '?')}", flush=True)

            if started and b'\x00[metadata]\n' in data:
                data, did_inject = replace_metadata_section(data)
                if did_inject:
                    injected_count += 1
                    meta = get_roon_metadata()
                    cover_size = os.path.getsize('/tmp/roon_cover.jpg') if os.path.exists('/tmp/roon_cover.jpg') else 0
                    print(f"{ts()} [INJECT] #{injected_count}: {meta.get('title', '?')} / {meta.get('artist', '?')} + {cover_size}b cover", flush=True)

            # Trace: find ALL occurrences of "Roon" in outgoing data
            if started and injected_count > 0 and injected_count < 10:
                idx = 0
                while True:
                    pos = data.find(b'Roon', idx)
                    if pos == -1:
                        break
                    ctx = data[max(0,pos-20):pos+20]
                    # Check if it's in text context (not random PCM bytes)
                    try:
                        ctx.decode('utf-8')
                        is_text = True
                    except:
                        is_text = False
                    if is_text:
                        print(f"{ts()} [ROON-LEAK] @{pos} in {len(data)}b: {ctx!r}", flush=True)
                    idx = pos + 1

            dst.sendall(data)
    except OSError as e:
        print(f"{ts()} [HQP->T8] error: {e}", flush=True)

def forward_t8_to_hqp(src, dst):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [T8->HQP] EOF", flush=True)
                break
            log_xml("T8->HQP", data)
            dst.sendall(data)
    except OSError as e:
        print(f"{ts()} [T8->HQP] error: {e}", flush=True)

if __name__ == '__main__':
    threading.Thread(target=discovery_responder, daemon=True).start()

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", NAA_PORT))
    listener.listen(5)
    print(f"{ts()} NAA proxy (Roon metadata): :43210 -> {T8_HOST}:43210", flush=True)

    while True:
        client, addr = listener.accept()
        print(f"{ts()} HQP connected from {addr}", flush=True)
        try:
            t8 = socket.create_connection((T8_HOST, NAA_PORT), timeout=5)
        except Exception as e:
            print(f"{ts()} T8 connect failed: {e}", flush=True)
            client.close()
            continue
        t1 = threading.Thread(target=forward_hqp_to_t8, args=(client, t8), daemon=True)
        t2 = threading.Thread(target=forward_t8_to_hqp, args=(t8, client), daemon=True)
        t1.start(); t2.start()
        t1.join(); t2.join()
        client.close(); t8.close()
        print(f"{ts()} Session ended", flush=True)
