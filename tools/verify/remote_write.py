"""Push 24 hours of synthetic host series for N machines to Mimir by remote
write (protobuf, snappy as literal blocks), presenting a client certificate.
Used by mimir_query_cert.sh (decision 0072, V4); no dependency beyond the
standard library. Machines are verify-00 … verify-NN; verify-07 has a gap
of two hours (from 10 h ago) and verify-08 stops 40 minutes ago.

    python3 remote_write.py URL CA CERT MACHINES NOW_SECONDS
"""

import math
import ssl
import struct
import sys
import urllib.request

URL, CA, CERT, N, NOW = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5])
STEP, SPAN = 30, 24 * 3600


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def field(num, payload):
    return varint(num << 3 | 2) + varint(len(payload)) + payload


def series(labels, samples):
    body = b"".join(field(1, field(1, k.encode()) + field(2, v.encode())) for k, v in sorted(labels.items()))
    for t, v in samples:
        body += field(2, b"\x09" + struct.pack("<d", v) + b"\x10" + varint(t * 1000))
    return field(1, body)


def snappy(data):
    out = bytearray(varint(len(data)))
    for i in range(0, len(data), 65536):
        chunk = data[i : i + 65536]
        out += bytes([61 << 2]) + struct.pack("<H", len(chunk) - 1) + chunk
    return bytes(out)


def times(vm):
    for t in range(NOW - SPAN, NOW + 1, STEP):
        if vm == 7 and NOW - 10 * 3600 <= t < NOW - 8 * 3600:
            continue
        if vm == 8 and t > NOW - 40 * 60:
            continue
        yield t


def machine(vm):
    name, ts = f"verify-{vm:02d}", list(times(vm))
    base = {"vm": name, "role": "worker", "job": "host", "instance": "host"}
    out = []
    for cpu in range(2):
        for i, mode in enumerate(("idle", "iowait", "user", "system")):
            share = (0.7, 0.05, 0.17, 0.08)[i]
            out.append(series({**base, "__name__": "node_cpu_seconds_total", "cpu": str(cpu), "mode": mode},
                              [(t, (t - NOW + SPAN) * share) for t in ts]))
    total = 16 * 2**30
    out.append(series({**base, "__name__": "node_memory_MemTotal_bytes"}, [(t, total) for t in ts]))
    out.append(series({**base, "__name__": "node_memory_MemAvailable_bytes"},
                      [(t, total * (0.5 + 0.3 * math.sin(t / 3600 + vm))) for t in ts]))
    for mp in ("spill", "docker"):
        out.append(series({**base, "__name__": "node_filesystem_size_bytes", "mountpoint": mp}, [(t, 1e11) for t in ts]))
        out.append(series({**base, "__name__": "node_filesystem_avail_bytes", "mountpoint": mp},
                          [(t, 1e11 * (0.6 - 0.2 * (t - NOW + SPAN) / SPAN)) for t in ts]))
    for dev in ("eth0", "eth1"):
        for d in ("receive", "transmit"):
            out.append(series({**base, "__name__": f"node_network_{d}_bytes_total", "device": dev},
                              [(t, (t - NOW + SPAN) * 1e5 * (vm + 1)) for t in ts]))
    out.append(series({**base, "__name__": "node_boot_time_seconds"}, [(t, NOW - 5 * 86400) for t in ts]))
    return b"".join(out)


ctx = ssl.create_default_context(cafile=CA)
ctx.load_cert_chain(CERT)
sent = 0
for vm in range(N):
    body = snappy(machine(vm))
    req = urllib.request.Request(URL, data=body, method="POST", headers={
        "Content-Encoding": "snappy", "Content-Type": "application/x-protobuf", "X-Prometheus-Remote-Write-Version": "0.1.0"})
    with urllib.request.urlopen(req, context=ctx, timeout=120) as r:
        if r.status // 100 != 2:
            sys.exit(f"push {vm}: {r.status}")
    sent += len(body)
print(f"pushed {N} machines, {sent} bytes")
