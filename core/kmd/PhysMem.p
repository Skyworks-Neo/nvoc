// PhysMem - Physical memory access primitives for nvoc's kmd channel.
// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Built with the official toolchain (compuphase pawncc 4.1.7152, same as the
// upstream PawnIO.Modules CI):
//
//   pawncc PhysMem.p -i<PawnIO>/pawn/include -C64 -;+ -(+ -p
//
// The driver only accepts signed blobs on the Official edition; see
// core/kmd/README.md for the blob format and the three options to satisfy the
// signature gate.

#include <pawnio.inc>

NTSTATUS:main() {
    // Upstream Echo.p notes a compiler/interpreter bug when a module references
    // only a single native; this module already uses several physical_* natives,
    // the version probe is belt-and-braces (and a load-time liveness signal).
    new version = get_version();
    if (version == 0)
        return STATUS_UNSUCCESSFUL;
    return STATUS_SUCCESS;
}

public NTSTATUS:unload() {
    return STATUS_SUCCESS;
}

// in[0] = physical address; out[0] = value. Any alignment is accepted.
DEFINE_IOCTL_SIZED(ioctl_read_phys_qword, 1, 1) {
    return physical_read_qword(in[0], out[0]);
}

DEFINE_IOCTL_SIZED(ioctl_read_phys_dword, 1, 1) {
    new value;
    new NTSTATUS:status = physical_read_dword(in[0], value);
    out[0] = value;
    return status;
}

// in[0] = 4 KiB-aligned physical address; out[0..511] = page contents.
DEFINE_IOCTL_SIZED(ioctl_read_phys_page, 1, 512) {
    new pa = in[0];
    if ((pa & 0xFFF) != 0)
        return STATUS_INVALID_PARAMETER;
    for (new i = 0; i < 512; i++) {
        new NTSTATUS:status = physical_read_qword(pa + i * 8, out[i]);
        if (!NT_SUCCESS(status))
            return status;
    }
    return STATUS_SUCCESS;
}

// Write side: present for the follow-up write experiments; the current nvoc
// channel is read-only and does not call these yet.
DEFINE_IOCTL_SIZED(ioctl_write_phys_dword, 2, 0) {
    return physical_write_dword(in[0], in[1]);
}

DEFINE_IOCTL_SIZED(ioctl_write_phys_word, 2, 0) {
    return physical_write_word(in[0], in[1]);
}

DEFINE_IOCTL_SIZED(ioctl_write_phys_byte, 2, 0) {
    return physical_write_byte(in[0], in[1]);
}
