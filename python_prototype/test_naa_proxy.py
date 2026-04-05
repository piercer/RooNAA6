import struct
import naa_proxy
from naa_proxy import replace_metadata_section


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_metadata_buffer(fields: dict, prefix=b'', suffix=b'') -> bytes:
    """Build a buffer with PCM-like prefix, metadata section, and suffix."""
    meta = '[metadata]\n'
    for k, v in fields.items():
        meta += f'{k}={v}\n'
    return prefix + b'\x00' + meta.encode('utf-8') + b'\x00' + suffix


HQP_META = {
    'bitrate': '2116800',
    'bits': '24',
    'channels': '2',
    'float': '0',
    'samplerate': '44100',
    'sdm': '0',
    'song': 'Roon',
}

ROON_META = {
    'title': 'Test Song',
    'artist': 'Test Artist',
    'album': 'Test Album',
}


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_replace_metadata_no_picture(monkeypatch):
    """Existing behavior: metadata replaced, no picture, same byte count."""
    monkeypatch.setattr(naa_proxy, 'get_roon_metadata', lambda: ROON_META)

    buf = make_metadata_buffer(HQP_META, prefix=b'\xab' * 100)

    result, did_inject = replace_metadata_section(buf, jpeg_data=None)

    assert did_inject
    assert len(result) == len(buf), "Without picture, byte count must be preserved"
    assert b'[metadata]' in result


def test_inject_jpeg_after_metadata(monkeypatch):
    """JPEG bytes are spliced between metadata null and trailing data."""
    monkeypatch.setattr(naa_proxy, 'get_roon_metadata', lambda: ROON_META)

    # Suffix simulates the next frame header that follows metadata
    frame_hdr = b'\x11\x00\x00\x00\x00\x40\x01\x00'
    buf = make_metadata_buffer(HQP_META, prefix=b'\xab' * 100, suffix=frame_hdr)

    fake_jpeg = b'\xff\xd8\xff\xe0' + b'\x00' * 50 + b'\xff\xd9'

    result, did_inject = replace_metadata_section(buf, jpeg_data=fake_jpeg)

    assert did_inject
    # Find the metadata null terminator
    meta_end = result.find(b'[metadata]')
    assert meta_end != -1
    # After the section's null terminator, JPEG should appear
    null_after_meta = result.find(b'\x00', meta_end + 11)
    after_null = result[null_after_meta + 1:]
    assert after_null.startswith(fake_jpeg), "JPEG must follow metadata null"
    # Frame header must follow JPEG
    assert after_null[len(fake_jpeg):] == frame_hdr, "Frame header must follow JPEG"


def test_inject_jpeg_metadata_at_buffer_end(monkeypatch):
    """When metadata null is the last byte, JPEG is appended."""
    monkeypatch.setattr(naa_proxy, 'get_roon_metadata', lambda: ROON_META)

    # No suffix — metadata null is last byte (common case from captures)
    buf = make_metadata_buffer(HQP_META, prefix=b'\xab' * 100, suffix=b'')

    fake_jpeg = b'\xff\xd8\xff\xe0' + b'\x00' * 50 + b'\xff\xd9'

    result, did_inject = replace_metadata_section(buf, jpeg_data=fake_jpeg)

    assert did_inject
    assert result.endswith(fake_jpeg), "JPEG must be appended when null is last byte"


def test_no_jpeg_when_none(monkeypatch):
    """When jpeg_data is None, no picture is injected (backward compat)."""
    monkeypatch.setattr(naa_proxy, 'get_roon_metadata', lambda: ROON_META)

    frame_hdr = b'\x11\x00\x00\x00\x00\x40\x01\x00'
    buf = make_metadata_buffer({'song': 'Test'}, prefix=b'\xab' * 20, suffix=frame_hdr)

    result, did_inject = replace_metadata_section(buf, jpeg_data=None)

    assert did_inject
    assert result.endswith(frame_hdr), "Without JPEG, trailing data unchanged"
