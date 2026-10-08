// Apply symbols from a DIA2Dump text file (produced with `DIA2Dump.exe -p <pdb>`) to the
// current program. Each PublicSymbol line has:
//   PublicSymbol: [XXXXXXXX][SSSS:OOOOOOOO] mangled(demangled)
// where the first [] is the image-base-relative RVA and the second is section:offset.
// We add currentProgram's imageBase to the RVA to get the VA and rename the function there.
//
// Uses the DEMANGLED name (parenthesized) when available - richer than the mangled form.
//
// The path can be overridden via System property "dia.dump" or the script arg;
// otherwise it defaults based on the current program's name.
//@category Saboteur
import java.io.*;
import java.util.*;
import java.util.regex.*;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.SourceType;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolUtilities;

public class ApplyDia2DumpSymbols extends GhidraScript {

    private static String defaultForProgram(String name) {
        // Combined DIA2Dump output: -p (publics: RTTI+vtables+exports) + -fn (per-compiland
        // functions including statics). Covers strictly more than -p alone.
        String lower = name.toLowerCase();
        String base = "C:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/output/pdb_dumps/";
        if (lower.contains("wildstar_p")) return base + "wildstar_p_pdb_combined.txt";
        if (lower.contains("wildstar_d")) return base + "wildstar_d_pdb_combined.txt";
        if (lower.contains("saboteur"))   return base + "saboteur_pdb_combined.txt";
        return null;
    }

    @Override
    public void run() throws Exception {
        String programName = currentProgram.getName();
        String dumpPath = System.getProperty("dia.dump");
        String[] argv = getScriptArgs();
        if (dumpPath == null && argv != null && argv.length > 0) dumpPath = argv[0];
        if (dumpPath == null) dumpPath = defaultForProgram(programName);
        if (dumpPath == null) {
            printerr("no DIA dump path for program: " + programName);
            return;
        }
        File f = new File(dumpPath);
        if (!f.exists()) { printerr("dump not found: " + dumpPath); return; }
        long imageBase = currentProgram.getImageBase().getOffset();
        println("ApplyDia2Dump: program=" + programName + " imageBase=0x" + Long.toHexString(imageBase));
        println("ApplyDia2Dump: reading " + dumpPath);

        // PublicSymbol: [XXXXXXXX][SSSS:OOOOOOOO] mangled(demangled)
        // Some lines are FunctionSymbol/SymTagFunction — accept those too via loose pattern.
        Pattern pat = Pattern.compile("^(?:PublicSymbol|Function|SymTag\\w+):\\s+\\[([0-9a-fA-F]{8})\\]\\[[0-9a-fA-F]{4}:[0-9a-fA-F]+\\]\\s+(\\S+?)(?:\\s*\\((.*)\\))?\\s*$");

        FunctionManager fm = currentProgram.getFunctionManager();
        Memory memory = currentProgram.getMemory();

        int parsed = 0, renamed = 0, labeled = 0, skipped_addr = 0, skipped_bad_name = 0, skipped_other = 0;
        try (BufferedReader r = new BufferedReader(new FileReader(f))) {
            String line;
            while ((line = r.readLine()) != null) {
                Matcher m = pat.matcher(line);
                if (!m.find()) continue;
                parsed++;
                long rva = Long.parseUnsignedLong(m.group(1), 16);
                String mangled = m.group(2);
                // Use the mangled name only. Demangled names contain Ghidra-invalid chars
                // (backticks, spaces, angle brackets, parens) that Ghidra's SymbolUtilities.validateName
                // rejects. Ghidra's own Microsoft demangler handles the mangled name at display time.
                String name = mangled;

                long va = imageBase + rva;
                Address a;
                try { a = toAddr(va); } catch (Exception e) { skipped_other++; continue; }
                if (a == null || !memory.contains(a)) { skipped_addr++; continue; }

                Function fn = fm.getFunctionAt(a);
                try {
                    // If there's no function at this VA, try to create one first (many DIA -fn
                    // entries are static-scope helpers Ghidra didn't discover). Ignore failures -
                    // will land as labels instead.
                    if (fn == null) {
                        try {
                            fn = createFunction(a, null);
                        } catch (Exception e) {
                            fn = null;
                        }
                    }
                    if (fn != null) {
                        String cur = fn.getName();
                        boolean isAuto = cur.startsWith("FUN_") || cur.startsWith("Function_") || cur.startsWith("thunk_FUN_");
                        if (isAuto) {
                            for (Symbol s : currentProgram.getSymbolTable().getSymbols(a)) {
                                if (s.getName().equals(name) && !s.equals(fn.getSymbol())) s.delete();
                            }
                            try {
                                fn.setName(name, SourceType.IMPORTED);
                                renamed++;
                            } catch (Exception inner) {
                                SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                                    currentProgram, a, null, name, SourceType.IMPORTED);
                                labeled++;
                            }
                        } else {
                            SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                                currentProgram, a, null, name, SourceType.IMPORTED);
                            labeled++;
                        }
                    } else {
                        SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                            currentProgram, a, null, name, SourceType.IMPORTED);
                        labeled++;
                    }
                } catch (Exception e) {
                    skipped_bad_name++;
                }
            }
        }
        println("ApplyDia2Dump: parsed=" + parsed + ", renamed=" + renamed
              + ", labeled=" + labeled + ", skipped_addr=" + skipped_addr
              + ", skipped_bad_name=" + skipped_bad_name
              + ", skipped_other=" + skipped_other);
    }
}
