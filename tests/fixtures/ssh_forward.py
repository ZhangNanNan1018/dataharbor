"""Local SSH protocol fixture. Temporary trust only; never reads user keys.

Requires the optional test-only Python package paramiko. The Rust ignored test
starts/stops this process and owns the temporary directory passed on the CLI.
"""
import pathlib
import socket
import sys
import threading

import paramiko


class Server(paramiko.ServerInterface):
    def check_auth_password(self, username, password):
        if username == "fixture" and password == "fixture":
            return paramiko.AUTH_SUCCESSFUL
        return paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return "password"

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        return paramiko.OPEN_SUCCEEDED


def echo(channel):
    try:
        while True:
            data = channel.recv(16384)
            if not data:
                channel.shutdown_write()
                break
            channel.sendall(data)
    except (OSError, EOFError):
        pass
    finally:
        channel.close()


key = paramiko.RSAKey.generate(2048)
listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen()
port = listener.getsockname()[1]
pathlib.Path(sys.argv[1]).write_text(
    f"[127.0.0.1]:{port} {key.get_name()} {key.get_base64()}\n", encoding="utf-8"
)
print(port, flush=True)
client, _ = listener.accept()
with paramiko.Transport(client) as transport:
    # Avoid Paramiko/libssh2's group-exchange interoperability differences; the
    # fixture exercises forwarding over a modern mutually supported KEX.
    transport.get_security_options().kex = ("diffie-hellman-group14-sha256",)
    transport.add_server_key(key)
    transport.start_server(server=Server())
    while transport.is_active():
        channel = transport.accept(timeout=0.2)
        if channel is not None:
            threading.Thread(target=echo, args=(channel,), daemon=True).start()
