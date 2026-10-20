#!/usr/bin/env python3
"""Decode the obfuscated NVAPI interface-ID table of mVolt+.exe 0.50.5.

The compat-report generator (sub_140013E90) walks two interface tables:
  - plain table @VA 0x1402C4BB8: 7 entries {u32 id; qword name; qword access} (24-byte stride)
  - obfuscated table @VA 0x1402C4860: 35 entries {qword name; u8 idx; qword access}
    where id = vm_run(prog=base+18*idx, key=u32 base+4*idx), base @VA 0x1402EE460

vm_run (sub_140174A60): keystream = splitmix64-finalizer(seed - 0x61C8864680B583EB*i) >> 56,
seed = key | 0xA53B112800000000; byte at pos p is XORed with ks[p+1]. Opcodes:
  0x09 acc=imm32 | 0x58 acc=rol32(acc,u8) | 0x93 acc+=imm32 | 0xCE acc^=imm32 | 0xCF return acc
imm32 (sub_1401748D0): 4 little-endian bytes at pos p XORed with ks[p+1..p+4].

All bytes are read straight from the PE file on disk (RVA -> file offset via the
section table) so the decode is independently verifiable of any tool session.
"""

import struct
import sys

EXE = "mVolt+.exe"
IMAGE_BASE = 0x140000000

# ---- PE section table (manual, no deps) ------------------------------------

def sections(data):
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe_off:pe_off + 4] == b"PE\x00\x00"
    nsec = struct.unpack_from("<H", data, pe_off + 6)[0]
    opt_size = struct.unpack_from("<H", data, pe_off + 20)[0]
    table = pe_off + 24 + opt_size
    out = []
    for i in range(nsec):
        e = table + 40 * i
        name = data[e:e + 8].rstrip(b"\x00").decode()
        vsize, va, rsize, roff = struct.unpack_from("<IIII", data, e + 8)
        out.append((name, va, vsize, roff, rsize))
    return out


def va_to_off(sections_, va):
    rva = va - IMAGE_BASE
    for name, sva, vsize, roff, rsize in sections_:
        if sva <= rva < sva + max(vsize, rsize):
            return roff + (rva - sva)
    raise ValueError(f"VA {va:#x} not mapped")


# ---- splitmix-style keystream ----------------------------------------------

G = 0x61C8864680B583EB
M1, M2, M3 = 0xBF58476D1CE4E5B9, 0x94D049BB133111EB, 0xA53B112800000000
MASK = (1 << 64) - 1


def ks_byte(seed, i):
    """keystream byte for 1-based position i (mirrors sub_140174A60 inlined expr)"""
    x = (seed - G * i) & MASK
    x = (x ^ (x >> 30)) & MASK
    x = (M1 * x) & MASK
    x = (x ^ (x >> 27)) & MASK
    x = (M2 * x) & MASK
    return (x >> 56) & 0xFF


def rol32(v, n):
    n &= 31
    return ((v << n) | (v >> (32 - n))) & 0xFFFFFFFF if n else v


def vm_run(prog, key):
    seed = (key | M3) & MASK
    acc, pos, log = 0, 0, []
    while True:
        if pos >= len(prog):
            return None, "overrun"
        op = prog[pos] ^ ks_byte(seed, pos + 1)
        pos += 1
        if op == 0x09 or op == 0x93 or op == 0xCE:
            if pos + 4 > len(prog):
                return None, "imm overrun"
            imm = 0
            for k in range(4):
                imm |= (prog[pos + k] ^ ks_byte(seed, pos + k + 1)) << (8 * k)
            pos += 4
            if op == 0x09:
                acc = imm; log.append(f"acc={imm:#x}")
            elif op == 0x93:
                acc = (acc + imm) & 0xFFFFFFFF; log.append(f"acc+={imm:#x}")
            else:
                acc ^= imm; log.append(f"acc^{imm:#x}")
        elif op == 0x58:
            if pos >= len(prog):
                return None, "rot overrun"
            amt = prog[pos] ^ ks_byte(seed, pos + 1)
            pos += 1
            acc = rol32(acc, amt); log.append(f"rol {amt}")
        elif op == 0xCF:
            log.append("end")
            return acc, "; ".join(log)
        else:
            return None, f"bad opcode {op:#x} at {pos - 1}"


def rd_cstr(data, off, maxlen=96):
    end = data.find(b"\x00", off, off + maxlen)
    return data[off:end].decode("utf-8", "replace")


def main():
    data = open(EXE, "rb").read()
    secs = sections(data)

    # ---- obfuscated VM region: keys @ 0x1402EE460, programs @ 0x1402EE6E0 ---
    key_base = va_to_off(secs, 0x1402EE460)
    prog_base = va_to_off(secs, 0x1402EE6E0)
    keys = data[key_base:key_base + 4 * 0x23]
    progs = data[prog_base:prog_base + 18 * 0x23]

    # ---- name/access string tables -----------------------------------------
    names_off = va_to_off(secs, 0x1402C4860)   # obfuscated table (35 entries)
    plain_off = va_to_off(secs, 0x1402C4BB8)   # plain table (7 entries)
    acc_a = rd_cstr(data, va_to_off(secs, 0x1402C3BC8))
    acc_b = rd_cstr(data, va_to_off(secs, 0x1402C3C90))

    print(f"access string A @0x1402C3BC8 = {acc_a!r}")
    print(f"access string B @0x1402C3C90 = {acc_b!r}")
    print()

    print("=== plain table (7 entries, direct IDs) ===")
    for k in range(7):
        e = plain_off + 24 * k
        name_p, ident, _pad, acc_p = struct.unpack_from("<QIIQ", data, e - 8)
        name = rd_cstr(data, va_to_off(secs, name_p)) if name_p else "<none>"
        acc = rd_cstr(data, va_to_off(secs, acc_p))
        print(f"  0x{ident:08X}  {name:<40} {acc}")

    print()
    print("=== obfuscated table (35 entries, VM-decoded) ===")
    ok = 0
    ids = []
    for k in range(35):
        e = names_off + 24 * k
        name_p, idx = struct.unpack_from("<qB", data, e)
        acc_p = struct.unpack_from("<q", data, e + 16)[0]
        prog = progs[18 * idx:18 * idx + 18]
        key = struct.unpack_from("<I", keys, 4 * idx)[0]
        ident, trace = vm_run(prog, key)
        name = rd_cstr(data, va_to_off(secs, name_p))
        acc = rd_cstr(data, va_to_off(secs, acc_p))
        tag = f"0x{ident:08X}" if ident is not None else f"FAIL({trace})"
        if ident is not None:
            ok += 1
            ids.append((idx, ident, name, acc))
        print(f"  idx {idx:02d} (k={k:02d})  {tag:<12} {name:<8} {acc}   [{trace}]")
    print(f"\ndecoded {ok}/35")

    # sanity: dump the VM programs of the first and last entries for the report
    print("\n=== sample programs ===")
    for idx in (0, 1, 0x22):
        prog = progs[18 * idx:18 * idx + 18]
        key = struct.unpack_from("<I", keys, 4 * idx)[0]
        print(f"  idx {idx:02d}: prog={prog.hex()} key={key:#010x}")


if __name__ == "__main__":
    sys.exit(main())
