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

def load_cover_art():
    """Read current cover art JPEG from disk. Returns bytes or None."""
    try:
        with open('/tmp/roon_cover.jpg', 'rb') as f:
            data = f.read()
        if data[:2] == b'\xff\xd8' and len(data) > 100 and len(data) <= 80000:
            return data
        if data[:2] == b'\xff\xd8' and len(data) > 80000:
            print(f"{ts()} [COVER] skipped: {len(data)}b > 80KB limit", flush=True)
    except (OSError, IOError):
        pass
    return None

def patch_frame_header(data, jpeg_len):
    """Patch the NAA v6 frame header to include picture length.

    Frame header format (32 bytes, little-endian):
      offset 0:  type bitmask (0x01=PCM, 0x04=PIC, 0x08=META, 0x10=POS)
      offset 4:  PCM data length
      offset 8:  position section length
      offset 12: metadata section length
      offset 16: picture data length
      offset 20: padding (zeros)

    We need to:
      - Set PIC bit (0x04) in the type byte
      - Write JPEG length at offset 16-19
    """
    buf = bytearray(data)
    # Set PIC bit in type byte
    buf[0] = buf[0] | 0x04
    # Write picture length
    struct.pack_into('<I', buf, 16, jpeg_len)
    return bytes(buf)

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

    new_content = f'song={title}\nartist={artist}\nalbum={album}\n'.encode('utf-8')

    if len(new_content) > target_len:
        new_content = f'song={title}\nartist={artist}\n'.encode('utf-8')
    if len(new_content) > target_len:
        max_title = target_len - len(f'song=\nartist={artist}\n')
        new_content = f'song={title[:max_title]}\nartist={artist}\n'.encode('utf-8')

    # Pad to exact target length
    if len(new_content) < target_len:
        padding = target_len - len(new_content)
        new_content = new_content[:-1] + b' ' * padding + b'\n'
    elif len(new_content) > target_len:
        new_content = new_content[:target_len - 1] + b'\n'

    before_meta = data[:section_start]
    after_null = data[section_end + 1:]

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
        mreq = struct.pack("4s4s", socket.inet_aton(addr), socket.inet_aton("192.168.30.212"))
        try:
            sock.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
        except OSError:
            pass
    print(f"{ts()} [discovery] listening on :43210 (mcast on 192.168.30.212)", flush=True)
    while True:
        try:
            data, addr = sock.recvfrom(4096)
            print(f"{ts()} [discovery] UDP from {addr}: {data[:80]}", flush=True)
            if b"discover" in data and b"network audio" in data:
                sock.sendto(DISCOVER_RESPONSE, addr)
                print(f"{ts()} [discovery] responded to {addr}", flush=True)
        except OSError:
            pass

def forward_hqp_to_t8(src, dst):
    started = False
    injected_count = 0
    header_patched = False
    last_image_key = None
    cached_jpeg = None
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [HQP->T8] EOF", flush=True)
                break
            log_xml("HQP->T8", data)

            if b'type="start"' in data and b'result=' not in data:
                started = True
                header_patched = False
                injected_count = 0
                meta = get_roon_metadata()
                # Only load new cover art if image_key changed
                image_key = meta.get("image_key", "")
                if image_key and image_key != last_image_key:
                    last_image_key = image_key
                    cached_jpeg = load_cover_art()
                    if cached_jpeg:
                        print(f"{ts()} [HQP->T8] start: {meta.get('artist', '?')} - {meta.get('title', '?')} (new cover {len(cached_jpeg)}b)", flush=True)
                    else:
                        print(f"{ts()} [HQP->T8] start: {meta.get('artist', '?')} - {meta.get('title', '?')} (no cover)", flush=True)
                else:
                    print(f"{ts()} [HQP->T8] start: {meta.get('artist', '?')} - {meta.get('title', '?')} (same cover)", flush=True)

            if started and b'\x00[metadata]\n' in data:
                data, did_inject = replace_metadata_section(data, jpeg_data=None)
                if did_inject:
                    injected_count += 1
                    if injected_count <= 3 or injected_count % 50 == 0:
                        meta = get_roon_metadata()
                        print(f"{ts()} [INJECT] #{injected_count}: {meta.get('title', '?')} / {meta.get('artist', '?')}", flush=True)

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
    print(f"{ts()} NAA proxy (Roon metadata + cover art): :43210 -> {T8_HOST}:43210", flush=True)

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
