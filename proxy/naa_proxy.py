import socket, threading, datetime, struct, os, json, sys, time, re
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

DISCOVER_RESPONSE = (
    '<?xml version="1.0" encoding="utf-8"?>'
    '<networkaudio>'
    '<discover result="OK" name="RooNAA6 Proxy" version="eversolo naa" protocol="6" trigger="0">'
    'network audio'
    '</discover>'
    '</networkaudio>\n'
).encode("utf-8")

MCAST_ADDRS = ["224.0.0.199", "239.192.0.199"]

# Shared in-memory state between Roon listener thread and proxy thread
_metadata = {}
_cover_art = None
_meta_lock = threading.Lock()

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
    """Read current track metadata from shared state."""
    with _meta_lock:
        return dict(_metadata)

def load_cover_art():
    """Read current cover art JPEG from shared state. Returns bytes or None."""
    with _meta_lock:
        return _cover_art

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
    """Forward HQPlayer->T8 with frame-level metadata injection.

    First frame after start: inject Roon metadata (variable size, with format fields).
    Gapless track change: when Roon reports new track, inject metadata into next frame.
    Subsequent META frames: strip them so T8 keeps our injected metadata.
    """
    PHASE_HEADER = 0
    PHASE_PASS = 1
    PHASE_SKIP = 2

    phase = PHASE_HEADER
    pass_remaining = 0
    skip_remaining = 0
    pending_inject = None
    header_buf = b''
    bytes_per_sample = 4
    meta_template = b'bitrate=1411200\nbits=16\nchannels=2\nfloat=0\nsamplerate=44100\nsdm=0\nsong=Roon\n'
    injected_this_start = False
    last_injected_title = None
    frame_count = 0

    try:
        while True:
            data = src.recv(65536)
            if not data:
                print(f"{ts()} [HQP->T8] EOF", flush=True)
                break

            if phase == PHASE_HEADER and not header_buf and data.lstrip().startswith(b'<'):
                log_xml("HQP->T8", data)
                if b'type="start"' in data and b'result=' not in data:
                    phase = PHASE_HEADER
                    pass_remaining = 0
                    skip_remaining = 0
                    pending_inject = None
                    injected_this_start = False
                    last_injected_title = None
                    frame_count = 0
                    header_buf = b''
                    m_bits = re.search(rb'bits="(\d+)"', data)
                    m_rate = re.search(rb'rate="(\d+)"', data)
                    m_stream = re.search(rb'stream="(\w+)"', data)
                    if m_bits:
                        bits = int(m_bits.group(1))
                        bytes_per_sample = max(1, bits // 8)
                        rate = int(m_rate.group(1)) if m_rate else 44100
                        stream = m_stream.group(1).decode() if m_stream else 'pcm'
                        is_dsd = (stream == 'dsd')
                        if is_dsd:
                            # HQPlayer reports DSD64 base rate in metadata regardless of actual rate
                            meta_rate = 2822400
                        else:
                            meta_rate = rate
                        meta_template = (
                            f'bitrate={meta_rate * bits * 2}\n'
                            f'bits={bits}\nchannels=2\nfloat=0\n'
                            f'samplerate={meta_rate}\n'
                            f'sdm={1 if is_dsd else 0}\n'
                            f'song=Roon\n'
                        ).encode()
                        print(f"{ts()} [HQP->T8] start: {bytes_per_sample} bytes/sample, {stream} {rate}Hz", flush=True)
                dst.sendall(data)
                continue

            # Binary data — frame-level processing
            pos = 0
            out = bytearray()

            while pos < len(data):
                if phase == PHASE_HEADER:
                    # Check for XML message mid-buffer (e.g. stop/start between frames)
                    if not header_buf and data[pos:pos+1] == b'<':
                        if out:
                            dst.sendall(bytes(out))
                            out = bytearray()
                        xml_data = data[pos:]
                        log_xml("HQP->T8", xml_data)
                        if b'type="start"' in xml_data and b'result=' not in xml_data:
                            pass_remaining = 0
                            skip_remaining = 0
                            pending_inject = None
                            injected_this_start = False
                            last_injected_title = None
                            frame_count = 0
                            m_bits = re.search(rb'bits="(\d+)"', xml_data)
                            m_rate = re.search(rb'rate="(\d+)"', xml_data)
                            m_stream = re.search(rb'stream="(\w+)"', xml_data)
                            if m_bits:
                                bits = int(m_bits.group(1))
                                bytes_per_sample = max(1, bits // 8)
                                rate = int(m_rate.group(1)) if m_rate else 44100
                                stream = m_stream.group(1).decode() if m_stream else 'pcm'
                                is_dsd = (stream == 'dsd')
                                if is_dsd:
                                    meta_rate = 2822400
                                else:
                                    meta_rate = rate
                                meta_template = (
                                    f'bitrate={meta_rate * bits * 2}\n'
                                    f'bits={bits}\nchannels=2\nfloat=0\n'
                                    f'samplerate={meta_rate}\n'
                                    f'sdm={1 if is_dsd else 0}\n'
                                    f'song=Roon\n'
                                ).encode()
                                print(f"{ts()} [HQP->T8] start: {bytes_per_sample} bytes/sample, {stream} {rate}Hz", flush=True)
                        dst.sendall(xml_data)
                        break

                    need = 32 - len(header_buf)
                    take = min(need, len(data) - pos)
                    header_buf += data[pos:pos+take]
                    pos += take

                    if len(header_buf) == 32:
                        header = bytearray(header_buf)
                        header_buf = b''
                        pcm_len = struct.unpack_from('<I', header, 4)[0]
                        pos_len = struct.unpack_from('<I', header, 8)[0]
                        meta_len = struct.unpack_from('<I', header, 12)[0]
                        pic_len = struct.unpack_from('<I', header, 16)[0]
                        pcm_bytes = pcm_len * bytes_per_sample

                        has_meta = bool(header[0] & 0x08)
                        meta = get_roon_metadata()
                        title = meta.get("title", "")
                        frame_count += 1

                        if pcm_len > 1000000 or pos_len > 10000:
                            print(f"{ts()} [CORRUPT] header hex: {header.hex()}", flush=True)

                        if title and has_meta and not injected_this_start:
                            # First META frame after start: variable-size injection
                            lines = meta_template.split(b'\n')
                            format_lines = [l for l in lines if l and not l.startswith(b'song=')]
                            new_lines = format_lines + [
                                f'song={title}'.encode(),
                                f'artist={meta.get("artist","")}'.encode(),
                                f'album={meta.get("album","")}'.encode(),
                            ]
                            content = b'\n'.join(new_lines) + b'\n'
                            meta_section = b'[metadata]\n' + content + b'\x00'
                            jpeg = load_cover_art()

                            header[0] = header[0] | 0x08 | (0x04 if jpeg else 0)
                            struct.pack_into('<I', header, 12, len(meta_section))
                            struct.pack_into('<I', header, 16, len(jpeg) if jpeg else 0)

                            pending_inject = meta_section + (jpeg if jpeg else b'')
                            injected_this_start = True
                            last_injected_title = title
                            pass_remaining = pcm_bytes + pos_len
                            skip_remaining = meta_len + pic_len
                            cover_size = len(jpeg) if jpeg else 0
                            print(f"{ts()} [INJECT] {title} / {meta.get('artist', '?')} / {meta.get('album', '?')} + {cover_size}b cover", flush=True)

                        elif title and injected_this_start and title != last_injected_title:
                            # Gapless track change: Roon reports new track, inject metadata
                            # HQPlayer does exactly this — sends 0x1D frame at track boundaries
                            lines = meta_template.split(b'\n')
                            format_lines = [l for l in lines if l and not l.startswith(b'song=')]
                            new_lines = format_lines + [
                                f'song={title}'.encode(),
                                f'artist={meta.get("artist","")}'.encode(),
                                f'album={meta.get("album","")}'.encode(),
                            ]
                            content = b'\n'.join(new_lines) + b'\n'
                            meta_section = b'[metadata]\n' + content + b'\x00'
                            jpeg = load_cover_art()

                            header[0] = header[0] | 0x08 | (0x04 if jpeg else 0)
                            struct.pack_into('<I', header, 12, len(meta_section))
                            struct.pack_into('<I', header, 16, len(jpeg) if jpeg else 0)

                            pending_inject = meta_section + (jpeg if jpeg else b'')
                            last_injected_title = title
                            pass_remaining = pcm_bytes + pos_len
                            skip_remaining = meta_len + pic_len
                            cover_size = len(jpeg) if jpeg else 0
                            print(f"{ts()} [GAPLESS] {title} / {meta.get('artist', '?')} / {meta.get('album', '?')} + {cover_size}b cover", flush=True)

                        elif has_meta and injected_this_start:
                            # Mid-stream META refresh: strip to keep our metadata
                            header[0] = header[0] & ~0x08 & ~0x04
                            struct.pack_into('<I', header, 12, 0)
                            struct.pack_into('<I', header, 16, 0)
                            pending_inject = None
                            pass_remaining = pcm_bytes + pos_len
                            skip_remaining = meta_len + pic_len
                            print(f"{ts()} [STRIP] META refresh stripped (frame {frame_count})", flush=True)

                        elif injected_this_start and title and frame_count % 300 == 0:
                            # Periodic metadata refresh (~30s) to prevent T8 display revert
                            lines = meta_template.split(b'\n')
                            format_lines = [l for l in lines if l and not l.startswith(b'song=')]
                            new_lines = format_lines + [
                                f'song={title}'.encode(),
                                f'artist={meta.get("artist","")}'.encode(),
                                f'album={meta.get("album","")}'.encode(),
                            ]
                            content = b'\n'.join(new_lines) + b'\n'
                            meta_section = b'[metadata]\n' + content + b'\x00'

                            header[0] = header[0] | 0x08
                            struct.pack_into('<I', header, 12, len(meta_section))

                            pending_inject = meta_section
                            last_injected_title = title
                            pass_remaining = pcm_bytes + pos_len
                            skip_remaining = meta_len + pic_len
                            print(f"{ts()} [REFRESH] {title} (frame {frame_count})", flush=True)

                        else:
                            pending_inject = None
                            pass_remaining = pcm_bytes + pos_len + meta_len + pic_len
                            skip_remaining = 0

                        out.extend(header)
                        phase = PHASE_PASS

                elif phase == PHASE_PASS:
                    take = min(len(data) - pos, pass_remaining)
                    out.extend(data[pos:pos+take])
                    pos += take
                    pass_remaining -= take

                    if pass_remaining == 0:
                        if pending_inject:
                            out.extend(pending_inject)
                            pending_inject = None
                        if skip_remaining > 0:
                            phase = PHASE_SKIP
                        else:
                            phase = PHASE_HEADER

                elif phase == PHASE_SKIP:
                    take = min(len(data) - pos, skip_remaining)
                    pos += take
                    skip_remaining -= take
                    if skip_remaining == 0:
                        phase = PHASE_HEADER

            if out:
                dst.sendall(bytes(out))
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
    """Connects to Roon Core WebSocket, subscribes to transport, writes metadata to disk."""
    def __init__(self):
        self.ws = None
        self.reqid = 0
        self.callbacks = {}
        self._last_image_key = None

    def send_complete(self, request_id, status="Success"):
        """Send a MOO/1 COMPLETE response (terminates the request)."""
        msg = f'MOO/1 COMPLETE {status}\nRequest-Id: {request_id}\n\n'.encode()
        self.ws.send_binary(msg)

    def send_continue(self, request_id, status="Changed", body=None):
        """Send a MOO/1 CONTINUE response (keeps the subscription alive)."""
        header = f'MOO/1 CONTINUE {status}\nRequest-Id: {request_id}\n'
        if body is not None:
            content = json.dumps(body).encode('utf-8')
            header += f'Content-Length: {len(content)}\nContent-Type: application/json\n'
            msg = header.encode() + b'\n' + content
        else:
            msg = (header + '\n').encode()
        self.ws.send_binary(msg)

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
        self._core_id = body.get('core_id', '') if isinstance(body, dict) else ''
        print(f"{ts()} [roon] core: {body.get('display_name', '?')} v{body.get('display_version', '?')} id={self._core_id[:16]}", flush=True)

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
            "provided_services": ["com.roonlabs.pairing:1", "com.roonlabs.ping:1"],
            "required_services": ["com.roonlabs.transport:2"],
            "optional_services": [],
            "website": ""
        }
        if token:
            reg_info["token"] = token

        self.send_request("com.roonlabs.registry:1/register", reg_info)
        print(f"{ts()} [roon] registration sent", flush=True)

        self.ws.settimeout(60)
        while True:
            try:
                resp = self.ws.recv()
                first, headers, body = self.parse_response(resp)
                if first is None:
                    continue
                # Handle incoming REQUESTs from Roon Core (ping, pairing)
                if first.startswith('MOO/1 REQUEST'):
                    rid = headers.get('Request-Id')
                    if rid is not None:
                        if 'subscribe_pairing' in first:
                            self.send_continue(rid, "Changed", {"paired_core_id": self._core_id})
                        else:
                            self.send_complete(rid)
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
                continue
            except Exception as e:
                print(f"{ts()} [roon] error: {e}", flush=True)
                break

    def _save_metadata(self, zone, now_playing):
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

        metadata = {
            "title": title, "artist": artist, "album": album,
            "zone": zone_name, "image_key": image_key
        }

        if image_key and image_key != self._last_image_key:
            self._last_image_key = image_key
            self._download_cover(image_key)

        with _meta_lock:
            _metadata.clear()
            _metadata.update(metadata)
        print(f"{ts()} [roon] {artist} — {title} ({album})", flush=True)

    def _download_cover(self, image_key):
        global _cover_art
        url = f'http://{ROON_HOST}:{ROON_PORT}/api/image/{image_key}?scale=fit&width=250&height=250&format=image/jpeg'
        try:
            resp = urllib.request.urlopen(url, timeout=5)
            data = resp.read()
            if data[:2] == b'\xff\xd8' and len(data) > 100:
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
        print(f"{ts()} [roon] reconnecting in 5s...", flush=True)
        time.sleep(5)

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
