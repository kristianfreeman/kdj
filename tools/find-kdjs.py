#!/usr/bin/env python3
"""The kdjs on this network: listens for their hellos (see crates/kdj/src/net.rs)
for a few seconds and prints one line each, "ADDRESS NAME ROLE".

    tools/find-kdjs.py [SECONDS]
"""
import socket
import struct
import sys
import time

GROUP, PORT = "239.255.75.74", 47474

s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
if hasattr(socket, "SO_REUSEPORT"):
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
s.bind(("", PORT))
s.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, struct.pack("4s4s", socket.inet_aton(GROUP), socket.inet_aton("0.0.0.0")))
s.settimeout(0.5)

seen = {}
end = time.time() + float(sys.argv[1] if len(sys.argv) > 1 else 3)
while time.time() < end:
    try:
        b, (ip, _) = s.recvfrom(1500)
    except socket.timeout:
        continue
    if not b.startswith(b"kdj") or len(b) < 22 or b[4:5] != b"H":
        continue
    if b[:4] != b"kdj2":
        seen.setdefault(ip, f"{ip} ? (an older kdj: {b[:4].decode(errors='replace')})")
        continue
    r = b[5:]
    first, count = r[0], r[1]
    n = r[15]
    name = r[16 : 16 + n].decode(errors="replace")
    role = f"deck {chr(65 + first)}" if count else "library"
    seen[ip] = f"{ip} {name} {role}"
for line in seen.values():
    print(line)
