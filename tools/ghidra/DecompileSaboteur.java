// Export decompiled C for every function in The Saboteur (Saboteur.exe, WildStar/Odin engine).
// Sibling of DecompileAllFunctions.java (Mercs2) but generalized: no Mercs2 seed VAs, dumps
// every non-thunk function in any executable block, sorted by address.
// Each function is emitted once (dedup by entry) with a header line:
//   ==== <name> @0xADDR  size=N  callers=[...] ====
// Writes output/_ghidra_saboteur/saboteur_all_functions_decomp.txt
//
// @category Saboteur
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.Reference;
import ghidra.util.task.ConsoleTaskMonitor;

import java.io.File;
import java.io.PrintWriter;
import java.util.LinkedHashSet;
import java.util.Set;
import java.util.TreeMap;

public class DecompileSaboteur extends GhidraScript {
    private PrintWriter fp;
    private DecompInterface decomp;
    private ConsoleTaskMonitor mon;
    private final Set<Long> done = new LinkedHashSet<>();

    private String callersOf(Function f) {
        StringBuilder sb = new StringBuilder();
        int n = 0;
        for (Reference r : getReferencesTo(f.getEntryPoint())) {
            if (!r.getReferenceType().isCall()) continue;
            Function c = getFunctionContaining(r.getFromAddress());
            sb.append(String.format("0x%08x%s ", r.getFromAddress().getOffset(),
                c != null ? "(" + c.getName() + ")" : ""));
            if (++n >= 12) { sb.append("..."); break; }
        }
        return sb.toString().trim();
    }

    private void dump(Function f) {
        if (f == null) return;
        long key = f.getEntryPoint().getOffset();
        if (!done.add(key)) return;
        try {
            DecompileResults res = decomp.decompileFunction(f, 60, mon);
            fp.println("============================================================");
            fp.println(String.format("==== %s @0x%08x  size=%d  callers=[%s] ====",
                f.getName(), key, f.getBody().getNumAddresses(), callersOf(f)));
            if (res != null && res.decompileCompleted())
                fp.println(res.getDecompiledFunction().getC());
            else
                fp.println("  DECOMP FAIL");
        } catch (Exception e) { fp.println("  EXC " + e); }
    }

    @Override
    public void run() throws Exception {
        // Output directory is the script's one required argument (no machine path baked in):
        //   -postScript DecompileSaboteur.java <checkout>/output/_ghidra_saboteur
        String[] argv = getScriptArgs();
        if (argv.length != 1) {
            throw new IllegalArgumentException(
                "DecompileSaboteur.java needs exactly one script arg: the output directory "
                + "(e.g. <checkout>/output/_ghidra_saboteur); got " + argv.length);
        }
        File outDir = new File(argv[0]);
        if (!outDir.isDirectory() && !outDir.mkdirs()) {
            throw new IllegalStateException("cannot create output directory " + outDir);
        }
        String outPath = new File(outDir, "saboteur_all_functions_decomp.txt").getPath();
        fp = new PrintWriter(new File(outPath), "UTF-8");
        decomp = new DecompInterface();
        decomp.openProgram(currentProgram);
        mon = new ConsoleTaskMonitor();
        FunctionManager fm = currentProgram.getFunctionManager();

        long lo = Long.MAX_VALUE, hi = 0;
        for (MemoryBlock b : currentProgram.getMemory().getBlocks()) {
            if (b.isExecute()) {
                lo = Math.min(lo, b.getStart().getOffset());
                hi = Math.max(hi, b.getEnd().getOffset());
            }
        }
        TreeMap<Long, Function> work = new TreeMap<>();
        for (Function f : fm.getFunctions(true)) {
            long off = f.getEntryPoint().getOffset();
            if (off >= lo && off <= hi && !f.isThunk()) work.put(off, f);
        }

        fp.println(String.format("# Saboteur ALL export: %d functions", work.size()));
        int i = 0, total = work.size();
        for (Function f : work.values()) {
            dump(f);
            if (++i % 100 == 0) { println("decompiled " + i + "/" + total); fp.flush(); }
            if (mon.isCancelled()) break;
        }
        decomp.dispose();
        fp.close();
        println("done -> " + outPath + "  (" + done.size() + " functions)");
    }
}
