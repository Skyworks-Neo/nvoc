// Frida script to find GPU-Z's LIVE per-rail power source.
//
// STATUS: SUPERSEDED. This script was built on the (now-refuted) hypothesis that
// GPU-Z's per-rail watts come from a lazily-resolved NVAPI QueryInterface ID.
// Static RE of unpacked GPU-Z.exe proved the watts come from a WinRing0 kernel
// driver doing direct PCI/MMIO (IOCTL 0x800064A0, mutex Global\Access_PCI),
// entirely outside NVAPI — so no QueryInterface ID is the source, and this hook
// will never surface a [LAZY-NEW] watts candidate. Kept for reference / for
// investigating OTHER (non-watts) NVAPI IDs GPU-Z resolves lazily. See
// docs/gpuz-per-rail-investigation.md for the decisive finding.
//
// KEY INSIGHT: the prior WinDbg capture recorded 99 IDs at STARTUP. But GPU-Z
// may resolve some QueryInterface IDs lazily — only on first sensor refresh.
// A startup-only capture can MISS the live power-read ID entirely.
//
// This script logs QueryInterface IDs in two phases:
//   Phase 1 (first 3s): record all startup IDs into a baseline set.
//   Phase 2 (after 3s): log ONLY IDs seen for the FIRST TIME — these are IDs
//     resolved lazily during sensor refresh, the prime candidates for the live
//     power/voltage source (Board/Chip/MVDDC/PWR_SRC watts).
//
// Usage:
//   frida -p <gpuz_pid> -l docs/gpuz-power-source-frida.js
//   then let GPU-Z's sensors tab tick for ~10s.
//
// GPU-Z is 32-bit WoW64 -> uses nvapi.dll (cdecl, id = arg0).
// Also hooks nvapi64_impl.dll in case (x64 fastcall, id = ecx).

const seen = new Set();
let phase = 1;

setTimeout(() => {
    phase = 2;
    console.log('\n=== PHASE 2: logging NEW (lazy) IDs only — let sensors tick ===\n');
}, 3000);

function hook(moduleName, is64) {
    const m = Process.findModuleByName(moduleName);
    if (!m) return false;
    const qi = m.findExportByName('nvapi_QueryInterface');
    if (!qi) return false;

    Interceptor.attach(qi, {
        onEnter(args) {
            // x86 cdecl: arg0 = first stack arg. x64 fastcall: ecx = arg0.
            this.id = is64 ? this.context.rcx.toUInt32() : args[0].toUInt32();
        },
        onLeave(retval) {
            const key = '0x' + this.id.toString(16);
            if (phase === 1) {
                seen.add(key);
            } else if (!seen.has(key)) {
                seen.add(key);
                console.log('[LAZY-NEW] ' + key + ' -> ' + retval + '   (' + moduleName + ')');
            }
        },
    });
    console.log('[+] hooked ' + moduleName + '!nvapi_QueryInterface @ ' + qi);
    return true;
}

hook('nvapi.dll', false);
hook('nvapi64_impl.dll', true);
hook('nvapi64.dll', true);
hook('nvapi_impl.dll', false);

console.log('\n[+] Hooked. Let GPU-Z run; watch sensors tab ~10s.');
console.log('[+] [LAZY-NEW] lines = IDs resolved only at refresh = power source candidates.');
