#!/usr/bin/env python3
"""
Minimal Roon metadata listener.
Connects to Roon Core, registers as extension, subscribes to transport,
writes now-playing metadata to /tmp/roon_now_playing.json for the NAA proxy.
"""
import sys, os
sys.path.insert(0, os.path.expanduser('~/.local/lib/python3'))

import json, time, threading
import websocket

ROON_HOST = "192.168.30.23"
ROON_PORT = 9330
METADATA_FILE = "/tmp/roon_now_playing.json"
TOKEN_FILE = "/tmp/roon_token.json"

class RoonMetadata:
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

        # Split header from body
        sep = data.find(b'\n\n')
        if sep == -1:
            return None, None, None

        header_part = data[:sep].decode('utf-8')
        body_part = data[sep+2:]

        lines = header_part.split('\n')
        first_line = lines[0]  # MOO/1 COMPLETE Success or MOO/1 REQUEST name

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
        print("Connected to Roon Core")

        # Step 1: Get info
        self.send_request("com.roonlabs.registry:1/info")
        resp = self.ws.recv()
        first, headers, body = self.parse_response(resp)
        print(f"Core: {body.get('display_name', '?')} v{body.get('display_version', '?')}")

        # Step 2: Register extension
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
        print("Registration sent — you may need to authorize in Roon Settings > Extensions")

        # Main loop
        self.ws.settimeout(60)
        while True:
            try:
                resp = self.ws.recv()
                first, headers, body = self.parse_response(resp)

                if first is None:
                    continue

                # Handle registration response
                if body and isinstance(body, dict):
                    if "token" in body:
                        with open(TOKEN_FILE, "w") as f:
                            json.dump({"token": body["token"]}, f)
                        print("Registered and paired!")
                        # Subscribe to transport
                        self.send_request("com.roonlabs.transport:2/subscribe_zones",
                                        {"subscription_key": "zones"})

                    # Handle zone data
                    zones = body.get("zones") or body.get("zones_changed") or []
                    if zones:
                        for zone in zones:
                            np = zone.get("now_playing")
                            if np:
                                self.save_metadata(zone, np)

            except websocket.WebSocketTimeoutException:
                # Send keepalive
                continue
            except Exception as e:
                print(f"Error: {e}")
                import traceback
                traceback.print_exc()
                break

    def save_metadata(self, zone, now_playing):
        zone_name = zone.get("display_name", "unknown")

        # Only track the Einstein zone (Roon -> HQPlayer -> proxy -> T8 pipeline)
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
            "title": title,
            "artist": artist,
            "album": album,
            "zone": zone_name,
            "image_key": image_key
        }

        # Download cover art if image_key changed
        if image_key and image_key != self._last_image_key:
            self._last_image_key = image_key
            self._download_cover(image_key)

        with open(METADATA_FILE, "w") as f:
            json.dump(metadata, f)

        print(f"[{zone_name}] {artist} — {title} ({album}) img={image_key[:16] if image_key else 'none'}")

    def _download_cover(self, image_key):
        """Download cover art from Roon Core and save as JPEG."""
        import urllib.request
        url = f'http://{ROON_HOST}:{ROON_PORT}/api/image/{image_key}?scale=fit&width=250&height=250&format=image/jpeg'
        try:
            resp = urllib.request.urlopen(url, timeout=5)
            data = resp.read()
            with open('/tmp/roon_cover.jpg', 'wb') as f:
                f.write(data)
            print(f"Cover art saved: {len(data)} bytes")
        except Exception as e:
            print(f"Cover download failed: {e}")

if __name__ == "__main__":
    rm = RoonMetadata()
    while True:
        try:
            rm.run()
        except Exception as e:
            print(f"Connection error: {e}")
        print("Reconnecting in 5s...")
        time.sleep(5)
        rm = RoonMetadata()
