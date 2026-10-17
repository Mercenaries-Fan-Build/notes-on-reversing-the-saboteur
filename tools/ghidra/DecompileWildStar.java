// Full decomp export for a 2008 Xbox 360 PowerPC PE of the WildStar engine (WildStar_d.exe,
// WildStar_p.exe). Adapted from DecompileSaboteur.java: same shape and output format.
// Every non-thunk function in an executable block is emitted once, sorted by address, header:
//   ==== <name> @0xADDR  size=N  callers=[...] ====
// followed by the decompiled C. The script's one required argument is the output file
// (conventionally <checkout>/output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt).
//
// @category Saboteur
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
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

public class DecompileWildStar extends GhidraScript {
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
        String[] argv = getScriptArgs();
        if (argv.length != 1) {
            throw new IllegalArgumentException(
                "DecompileWildStar.java needs exactly one script arg: the output file "
                + "(e.g. <checkout>/output/_ghidra_wildstar/wildstar_d_all_functions_decomp.txt); got "
                + argv.length);
        }
        File outFile = new File(argv[0]);
        File outDir = outFile.getAbsoluteFile().getParentFile();
        if (!outDir.isDirectory() && !outDir.mkdirs()) {
            throw new IllegalStateException("cannot create output directory " + outDir);
        }
        String outPath = outFile.getPath();
        fp = new PrintWriter(outFile, "UTF-8");
        decomp = new DecompInterface();
        // Keep blocks the decompiler would prove unreachable: a value read through a virtual call
        // into a stack local is otherwise folded to its initial value, deleting the branch that tests
        // it (WSStreamBlock::LoadBlock's SBL magic check in WildStar_p.exe).
        DecompileOptions options = new DecompileOptions();
        options.grabFromProgram(currentProgram);
        options.setEliminateUnreachable(false);
        decomp.setOptions(options);
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

        fp.println(String.format("# %s ALL export: %d functions (PPC BE, ImageBase 0x%08x)",
            currentProgram.getName(), work.size(), currentProgram.getImageBase().getOffset()));
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
