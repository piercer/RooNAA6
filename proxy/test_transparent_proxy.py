import socket
import threading
import time
from transparent_proxy import forward

def socket_pair():
    """Return a connected (server_sock, client_sock) pair on loopback."""
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]
    client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    client.connect(("127.0.0.1", port))
    server, _ = srv.accept()
    srv.close()
    return server, client


def test_forward_passes_line_through(capsys):
    """Lines read from src arrive unchanged at dst."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "A→B"), daemon=True)
    t.start()

    src_client.sendall(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n")
    dst_client.settimeout(2)
    received = dst_client.recv(1024)
    assert received == b"<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n"

    src_client.close()
    t.join(timeout=2)


def test_forward_logs_xml_to_stdout(capsys):
    """Each forwarded line is printed to stdout with direction label."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "Roon→HQP"), daemon=True)
    t.start()

    src_client.sendall(b"<?xml version=\"1.0\" encoding=\"utf-8\"?><Stop/>\n")
    time.sleep(0.1)

    captured = capsys.readouterr()
    assert "[Roon→HQP]" in captured.out
    assert "<Stop/>" in captured.out

    src_client.close()
    t.join(timeout=2)


def test_forward_exits_on_src_close():
    """forward() returns when the source socket closes."""
    src_server, src_client = socket_pair()
    dst_server, dst_client = socket_pair()

    t = threading.Thread(target=forward, args=(src_server, dst_server, "A→B"), daemon=True)
    t.start()

    src_client.close()
    t.join(timeout=2)
    assert not t.is_alive(), "forward() should have returned after src closed"
