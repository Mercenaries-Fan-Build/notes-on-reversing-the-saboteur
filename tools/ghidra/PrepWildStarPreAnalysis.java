// Pre-analysis script for WildStar_d.exe (Xbox 360 PowerPC PE).
//
// Runs BEFORE Ghidra's auto-analysis. Does two jobs:
//   1) Disable "PDB Universal" analyzer — it crashes on this Xbox 360 PDB
//      (DefaultPdbApplicator.applyDataTypesAndMainSymbolsAnalysis:299).
//      We'll bring in symbols another way (linker-map join or DIA2Dump follow-up).
//   2) Walk the .pdata section — each 8-byte RUNTIME_FUNCTION entry has
//      { uint32_t BeginAddress (RVA); uint32_t packed_flags_lengths; } BE.
//      For every BeginAddress, ensure a function exists at ImageBase+BeginAddress.
//      This gives auto-analysis the full function boundary set (68,571 anchors)
//      instead of letting it discover only ~48k on its own.
//
// @category Saboteur
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.SourceType;
import ghidra.program.model.listing.Program;
import ghidra.app.plugin.core.analysis.AutoAnalysisManager;
import ghidra.framework.options.Options;

public class PrepWildStarPreAnalysis extends GhidraScript {

    @Override
    public void run() throws Exception {
        Program p = currentProgram;
        long imageBase = p.getImageBase().getOffset();
        println("PrepWildStar: imageBase=0x" + Long.toHexString(imageBase));

        // ---- (1) disable the crashing PDB Universal analyzer ----
        Options opts = p.getOptions(Program.ANALYSIS_PROPERTIES);
        int disabled = 0;
        for (String name : opts.getOptionNames()) {
            String lower = name.toLowerCase();
            if (lower.contains("pdb") && lower.contains("universal")) {
                try {
                    opts.setBoolean(name, false);
                    println("  disabled analyzer option: " + name);
                    disabled++;
                } catch (Exception e) {
                    println("  could not disable " + name + ": " + e);
                }
            }
        }
        println("PrepWildStar: disabled " + disabled + " PDB Universal analyzer option(s)");

        // ---- (2) read .pdata, create functions at every BeginAddress ----
        // On Xbox 360 PPC PEs .pdata stores the FULL VA in big-endian (verified against the raw
        // file: first 8 bytes at file offset 0x2aac00 are `82 34 00 00 40 00 09 03`, i.e.
        // BeginAddress=0x82340000 which is the .text start VA, prolog=0x40, flags/length packed).
        // DO NOT add imageBase again.
        Memory mem = p.getMemory();
        MemoryBlock pdata = mem.getBlock(".pdata");
        if (pdata == null) {
            println("PrepWildStar: no .pdata block; skipping function-creation pass");
            return;
        }
        long start = pdata.getStart().getOffset();
        long size = pdata.getSize();
        println("PrepWildStar: .pdata @ 0x" + Long.toHexString(start) + " size=" + size);

        byte[] buf = new byte[(int) size];
        mem.getBytes(pdata.getStart(), buf);

        FunctionManager fm = p.getFunctionManager();
        int created = 0;
        int alreadyThere = 0;
        int skippedBad = 0;
        int outOfRange = 0;
        int total = (int)(size / 8);
        for (int i = 0; i < total; i++) {
            int off = i * 8;
            // big-endian uint32 = full VA
            long va = ((long)(buf[off]   & 0xff) << 24)
                    | ((long)(buf[off+1] & 0xff) << 16)
                    | ((long)(buf[off+2] & 0xff) << 8)
                    | ((long)(buf[off+3] & 0xff));
            if (va == 0) continue;
            Address a;
            try { a = toAddr(va); } catch (Exception e) { skippedBad++; continue; }
            if (!mem.contains(a)) { outOfRange++; continue; }
            Function existing = fm.getFunctionAt(a);
            if (existing != null) { alreadyThere++; continue; }
            try {
                createFunction(a, null);   // let auto-analysis name / analyze later
                created++;
                if ((created % 5000) == 0) {
                    println("  created " + created + " functions so far ...");
                }
            } catch (Exception e) {
                skippedBad++;
            }
        }
        println("PrepWildStar: pdata entries=" + total
              + ", created=" + created
              + ", alreadyThere=" + alreadyThere
              + ", outOfRange=" + outOfRange
              + ", skipped=" + skippedBad);
    }
}
