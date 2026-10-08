// Force-apply Ghidra's Microsoft demangler to every named function.
// After ApplyMapSymbols/ApplyDia2Dump sets mangled names (`?Foo@Bar@@QAAXXZ`), Ghidra's
// auto-analysis DemanglerAnalyzer often skips them (already IMPORTED-source) so the function
// signature stays untyped. This script walks all functions and calls the demangler explicitly,
// which extracts return type + parameter types + calling convention and applies them to the fn.
//
// @category Saboteur
import ghidra.app.script.GhidraScript;
import ghidra.app.util.demangler.DemangledObject;
import ghidra.app.util.demangler.microsoft.MicrosoftDemangler;
import ghidra.app.util.demangler.DemanglerOptions;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.symbol.SourceType;
import ghidra.app.util.MemoryBlockUtils;

public class ApplyMsvcDemangler extends GhidraScript {

    @Override
    public void run() throws Exception {
        MicrosoftDemangler dm = new MicrosoftDemangler();
        DemanglerOptions opts = new DemanglerOptions();
        opts.setApplySignature(true);
        opts.setDoDisassembly(false);

        FunctionManager fm = currentProgram.getFunctionManager();
        long total = 0, tried = 0, ok = 0, err = 0, skipped = 0;
        for (Function fn : fm.getFunctions(true)) {
            total++;
            if (monitor.isCancelled()) break;
            Address a = fn.getEntryPoint();
            String cur = fn.getName();
            // Only try to demangle MSVC-style mangled names.
            // Ghidra sanitizes '?' to '_' when applied as symbols, so accept either prefix.
            String mangled = null;
            if (cur.startsWith("?")) {
                mangled = cur;
            } else if (cur.startsWith("_?")) {
                mangled = cur.substring(1);
            } else if (cur.startsWith("_") && cur.length() > 2 && cur.charAt(1) == '?') {
                mangled = cur.substring(1);
            }
            if (mangled == null) { skipped++; continue; }

            tried++;
            try {
                ghidra.app.util.demangler.MangledContext ctx =
                    dm.createMangledContext(mangled, opts, currentProgram, a);
                DemangledObject obj = dm.demangle(ctx);
                if (obj == null) { err++; continue; }
                boolean applied = obj.applyTo(currentProgram, a, opts, monitor);
                if (applied) ok++; else err++;
            } catch (Exception e) {
                err++;
            }
            if (total % 5000 == 0) {
                println("processed " + total + " (tried=" + tried + " ok=" + ok + " err=" + err + ")");
            }
        }
        println("ApplyMsvcDemangler: total=" + total + " tried=" + tried + " ok=" + ok + " err=" + err + " skipped=" + skipped);
    }
}
