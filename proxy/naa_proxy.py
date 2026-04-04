import socket, threading, datetime, struct, os, json, sys, time
sys.path.insert(0, os.path.expanduser('~/.local/lib/python3'))
import urllib.request
try:
    import websocket
except ImportError:
    websocket = None

T8_HOST = "192.168.30.109"
NAA_PORT = 43210
ROON_HOST = "192.168.30.23"
ROON_PORT = 9330
TOKEN_FILE = "/tmp/roon_token.json"

# Shared metadata state (written by Roon listener thread, read by proxy)
_metadata = {}
_cover_art = None
_meta_lock = threading.Lock()
_meta_version = 0
_force_roon_reconnect = threading.Event()

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
    """Read current track metadata from shared state (set by Roon listener thread)."""
    with _meta_lock:
        return dict(_metadata)

def load_cover_art():
    """Read current cover art JPEG from shared state. Returns bytes or None."""
    with _meta_lock:
        data = _cover_art
    if data and data[:2] == b'\xff\xd8' and 100 < len(data) <= 80000:
        return data
    if data and data[:2] == b'\xff\xd8' and len(data) > 80000:
        print(f"{ts()} [COVER] skipped: {len(data)}b > 80KB limit", flush=True)
    return None

_expand_log_count = 0
_merge_log_count = 0

def find_frame_header(data, meta_marker_pos):
    """Find the 32-byte NAA v6 frame header for the frame containing metadata.

    Searches backward from the metadata marker, validating each candidate
    by checking that header + PCM_LEN + POS_LEN points to the marker position.
    """
    for offset in range(32, min(meta_marker_pos + 1, 70000)):
        h = meta_marker_pos - offset
        if h < 0:
            break
        type_byte = data[h]
        if not (type_byte & 0x08) or (type_byte & ~0x1D):
            continue
        pcm_len = struct.unpack_from('<I', data, h + 4)[0]
        pos_len = struct.unpack_from('<I', data, h + 8)[0]
        if h + 32 + pcm_len + pos_len == meta_marker_pos:
            meta_len = struct.unpack_from('<I', data, h + 12)[0]
            pic_len = struct.unpack_from('<I', data, h + 16)[0]
            return h, pcm_len, pos_len, meta_len, pic_len
    return -1, 0, 0, 0, 0

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

    print(f"{ts()} [META] target_len={target_len}, title='{title}', artist='{artist}'", flush=True)

    new_content = f'song={title}\nartist={artist}\nalbum={album}\n'.encode('utf-8')

    if len(new_content) > target_len:
        new_content = f'song={title}\nartist={artist}\n'.encode('utf-8')
    if len(new_content) > target_len:
        new_content = f'song={title}\n'.encode('utf-8')

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

def merge_metadata_section(data, jpeg_data=None):
    """Merge Roon metadata into [metadata] section, preserving original fields (bitrate, bits, etc.).

    Unlike replace_metadata_section which replaces ALL fields, this keeps the original
    audio format fields and only overrides song/artist/album. Same byte count output.
    """
    global _merge_log_count
    marker = b'\x00[metadata]\n'
    mpos = data.find(marker)
    if mpos == -1:
        return data, False

    section_start = mpos + len(marker)
    section_end = data.find(b'\x00', section_start)
    if section_end == -1:
        return data, False

    target_len = section_end - section_start
    original_content = data[section_start:section_end]

    meta = get_roon_metadata()
    title = meta.get("title", "")
    if not title:
        return data, False

    artist = meta.get("artist", "")
    album = meta.get("album", "")

    # Parse original key=value pairs (preserves order)
    merged = {}
    for line in original_content.decode('utf-8', errors='replace').split('\n'):
        if '=' in line:
            key, val = line.split('=', 1)
            merged[key] = val

    _merge_log_count += 1
    if _merge_log_count <= 3:
        print(f"{ts()} [META] original fields: {list(merged.items())}", flush=True)

    # Override with Roon metadata
    merged['song'] = title
    if artist:
        merged['artist'] = artist
    if album:
        merged['album'] = album

    # Serialize — try full merge first, then drop fields to fit
    new_content = ''.join(f'{k}={v}\n' for k, v in merged.items()).encode('utf-8')

    if len(new_content) > target_len:
        # Drop album to save space
        trimmed = {k: v for k, v in merged.items() if k != 'album'}
        new_content = ''.join(f'{k}={v}\n' for k, v in trimmed.items()).encode('utf-8')

    if len(new_content) > target_len:
        # Drop artist too
        trimmed = {k: v for k, v in trimmed.items() if k != 'artist'}
        new_content = ''.join(f'{k}={v}\n' for k, v in trimmed.items()).encode('utf-8')

    if len(new_content) > target_len:
        # Truncate title as last resort
        avail = target_len - len(new_content) + len(title.encode('utf-8'))
        if avail > 10:
            trimmed['song'] = title[:avail - 3] + '...'
        else:
            trimmed['song'] = title[:avail]
        new_content = ''.join(f'{k}={v}\n' for k, v in trimmed.items()).encode('utf-8')

    if len(new_content) > target_len:
        new_content = new_content[:target_len - 1] + b'\n'

    # Pad to exact target length
    if len(new_content) < target_len:
        padding = target_len - len(new_content)
        new_content = new_content[:-1] + b' ' * padding + b'\n'

    if _merge_log_count <= 5:
        print(f"{ts()} [META] merge: {target_len}b, title='{title}', artist='{artist}'", flush=True)

    before_meta = data[:section_start]
    after_null = data[section_end + 1:]

    if jpeg_data:
        modified = before_meta + new_content + b'\x00' + jpeg_data + after_null
    else:
        modified = before_meta + new_content + b'\x00' + after_null
    return modified, True


def process_frame(frame, pcm_len, pos_len, meta_len, pic_len):
    """Process a complete NAA v6 frame: replace metadata with Roon data.

    Since we have the complete frame, the header is at offset 0 and we can
    update META_LEN directly after expanding the metadata section.
    """
    global _expand_log_count
    meta_offset = 32 + pcm_len + pos_len

    marker = b'\x00[metadata]\n'
    if frame[meta_offset:meta_offset + len(marker)] != marker:
        return frame, False

    section_start = meta_offset + len(marker)
    # Null separator is at meta_offset + meta_len (just after META section, not counted in meta_len)
    section_end = meta_offset + meta_len
    if section_end >= len(frame) or frame[section_end] != 0x00:
        return frame, False

    original_content = frame[section_start:section_end]
    old_len = len(original_content)

    # Parse original key=value pairs (preserves order in Python 3.7+)
    merged = {}
    for line in original_content.decode('utf-8', errors='replace').split('\n'):
        if '=' in line:
            key, val = line.split('=', 1)
            merged[key] = val

    meta = get_roon_metadata()
    title = meta.get("title", "")
    if not title:
        return frame, False

    merged['song'] = title
    artist = meta.get("artist", "")
    album = meta.get("album", "")
    if artist:
        merged['artist'] = artist
    if album:
        merged['album'] = album

    new_content = ''.join(f'{k}={v}\n' for k, v in merged.items()).encode('utf-8')
    size_diff = len(new_content) - old_len

    # Rebuild frame with new metadata
    new_frame = bytearray(frame[:section_start] + new_content + frame[section_end:])

    # Update META_LEN in header (offset 12)
    struct.pack_into('<I', new_frame, 12, meta_len + size_diff)

    _expand_log_count += 1
    if _expand_log_count <= 5:
        print(f"{ts()} [META] {old_len}→{len(new_content)}b, meta_len {meta_len}→{meta_len + size_diff}, keys={list(merged.keys())}", flush=True)
    return bytes(new_frame), True

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
    last_image_key = None
    cached_jpeg = None
    start_meta_version = 0
    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [HQP->T8] EOF", flush=True)
                break
            log_xml("HQP->T8", data)

            if b'type="start"' in data and b'result=' not in data:
                started = True
                injected_count = 0
                # Force Roon reconnect for fresh metadata
                with _meta_lock:
                    start_meta_version = _meta_version
                _force_roon_reconnect.set()
                meta = get_roon_metadata()
                image_key = meta.get("image_key", "")
                if image_key and image_key != last_image_key:
                    last_image_key = image_key
                    cached_jpeg = load_cover_art()
                art_info = f"cover {len(cached_jpeg)}b" if cached_jpeg else "no cover"
                print(f"{ts()} [HQP->T8] start: {meta.get('artist', '?')} - {meta.get('title', '?')} ({art_info})", flush=True)

            if started and b'\x00[metadata]\n' in data:
                # Wait for fresh metadata from Roon (up to 1.5s)
                deadline = time.time() + 1.5
                while time.time() < deadline:
                    with _meta_lock:
                        if _meta_version > start_meta_version:
                            break
                    time.sleep(0.05)
                with _meta_lock:
                    got_fresh = _meta_version > start_meta_version
                meta = get_roon_metadata()
                data, did_inject = replace_metadata_section(data, jpeg_data=None)
                if did_inject:
                    injected_count += 1
                    print(f"{ts()} [INJECT] #{injected_count} fresh={'Y' if got_fresh else 'N'}: {meta.get('title', '?')} / {meta.get('artist', '?')}", flush=True)

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

class RoonMetadata:
    """Connects to Roon Core WebSocket, subscribes to transport, updates shared metadata."""
    def __init__(self):
        self.ws = None
        self.reqid = 0
        self.callbacks = {}
        self._last_image_key = None

    def send_request(self, name, body=None, cb=None):
        rid = self.reqid
        self.reqid += 1
        header = f'MOO/1 REQUEST {name}\nRequest-Id: {rid}\n'
        if body is not None:
            content = json.dumps(body).encode('utf-8')
            header += f'Content-Length: {len(content)}\nContent-Type: application/json\n'
            msg = header.encode() + b'\n' + content
        else:
            msg = (header + '\n').encode()
        if cb:
            self.callbacks[rid] = cb
        self.ws.send_binary(msg)
        return rid

    def parse_response(self, data):
        if isinstance(data, str):
            data = data.encode('utf-8')
        sep = data.find(b'\n\n')
        if sep == -1:
            return None, None, None
        header_part = data[:sep].decode('utf-8')
        body_part = data[sep+2:]
        lines = header_part.split('\n')
        first_line = lines[0]
        headers = {}
        for line in lines[1:]:
            if ':' in line:
                k, v = line.split(':', 1)
                headers[k.strip()] = v.strip()
        body = None
        if body_part:
            try:
                body = json.loads(body_part)
            except:
                body = body_part
        return first_line, headers, body

    def run(self):
        self.ws = websocket.WebSocket()
        self.ws.connect(f'ws://{ROON_HOST}:{ROON_PORT}/api', timeout=10)
        print(f"{ts()} [roon] connected to Roon Core", flush=True)

        self.send_request("com.roonlabs.registry:1/info")
        resp = self.ws.recv()
        first, headers, body = self.parse_response(resp)
        print(f"{ts()} [roon] core: {body.get('display_name', '?')} v{body.get('display_version', '?')}", flush=True)

        token = None
        if os.path.exists(TOKEN_FILE):
            with open(TOKEN_FILE) as f:
                token = json.load(f).get("token")

        reg_info = {
            "extension_id": "com.roonaa6.metadata",
            "display_name": "RooNAA6 Metadata",
            "display_version": "1.0.0",
            "publisher": "RooNAA6",
            "email": "noreply@example.com",
            "provided_services": [],
            "required_services": ["com.roonlabs.transport:2"],
            "optional_services": [],
            "website": ""
        }
        if token:
            reg_info["token"] = token

        self.send_request("com.roonlabs.registry:1/register", reg_info)
        print(f"{ts()} [roon] registration sent", flush=True)

        self.ws.settimeout(1)
        _ka_count = 0
        while True:
            try:
                resp = self.ws.recv()
                first, headers, body = self.parse_response(resp)
                if first:
                    print(f"{ts()} [roon] <<< {first[:80]}", flush=True)
                if first is None:
                    continue
                if body and isinstance(body, dict):
                    if "token" in body:
                        with open(TOKEN_FILE, "w") as f:
                            json.dump({"token": body["token"]}, f)
                        print(f"{ts()} [roon] paired!", flush=True)
                        self.send_request("com.roonlabs.transport:2/subscribe_zones",
                                        {"subscription_key": "zones"})
                    zones = body.get("zones") or body.get("zones_changed") or []
                    if zones:
                        for zone in zones:
                            np = zone.get("now_playing")
                            if np:
                                self._save_metadata(zone, np)
            except websocket.WebSocketTimeoutException:
                if _force_roon_reconnect.is_set():
                    _force_roon_reconnect.clear()
                    print(f"{ts()} [roon] force reconnect requested", flush=True)
                    break
                continue
            except Exception as e:
                print(f"{ts()} [roon] error: {e}", flush=True)
                break

    def _save_metadata(self, zone, now_playing):
        global _cover_art, _meta_version
        zone_name = zone.get("display_name", "unknown")
        if zone_name != "Einstein":
            return
        three = now_playing.get("three_line", {})
        two = now_playing.get("two_line", {})
        one = now_playing.get("one_line", {})
        title = three.get("line1") or two.get("line1") or one.get("line1", "")
        artist = three.get("line2") or two.get("line2") or ""
        album = three.get("line3") or ""
        image_key = now_playing.get("image_key", "")

        with _meta_lock:
            _metadata.update({
                "title": title, "artist": artist, "album": album,
                "zone": zone_name, "image_key": image_key
            })
            _meta_version += 1

        if image_key and image_key != self._last_image_key:
            self._last_image_key = image_key
            self._download_cover(image_key)

        print(f"{ts()} [roon] {artist} — {title} ({album})", flush=True)

    def _download_cover(self, image_key):
        global _cover_art
        url = f'http://{ROON_HOST}:{ROON_PORT}/api/image/{image_key}?scale=fit&width=250&height=250&format=image/jpeg'
        try:
            resp = urllib.request.urlopen(url, timeout=5)
            data = resp.read()
            with _meta_lock:
                _cover_art = data
            print(f"{ts()} [roon] cover art: {len(data)}b", flush=True)
        except Exception as e:
            print(f"{ts()} [roon] cover download failed: {e}", flush=True)

def roon_listener_thread():
    """Roon metadata listener with auto-reconnect. Runs as daemon thread."""
    while True:
        try:
            rm = RoonMetadata()
            rm.run()
        except Exception as e:
            print(f"{ts()} [roon] connection error: {e}", flush=True)
        print(f"{ts()} [roon] reconnecting in 0.5s...", flush=True)
        time.sleep(0.5)

if __name__ == '__main__':
    if websocket:
        threading.Thread(target=roon_listener_thread, daemon=True).start()
    else:
        print(f"{ts()} WARNING: websocket-client not installed, Roon metadata disabled", flush=True)

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
