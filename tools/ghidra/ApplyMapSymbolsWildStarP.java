// Apply symbols from MSVC linker .map file to the current program.
// Parses the "Publics by Value" section and renames FUN_ functions.
//@category Analysis

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

public class ApplyMapSymbolsWildStarP extends GhidraScript {
    @Override
    public void run() throws Exception {
        // Map file path - adjust as needed
        String mapPath = "C:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files/Bin360/WildStar_p.map";

        File mapFile = new File(mapPath);
        if (!mapFile.exists()) {
            printerr("Map file not found: " + mapPath);
            return;
        }

        println("Parsing map file: " + mapPath);

        // Parse "Publics by Value" section
        // Format: 0005:000186e0   ?PoseMeshes@CharBonesMeshes@@QAAXXZ 823486e0 f   char:CharBonesMeshes.obj
        Pattern pattern = Pattern.compile(
            "^\\s*[0-9a-fA-F]{4}:[0-9a-fA-F]+\\s+(\\S+)\\s+([0-9a-fA-F]{8})");

        List<String[]> symbols = new ArrayList<>();
        boolean inPublics = false;

        try (BufferedReader reader = new BufferedReader(new FileReader(mapFile))) {
            String line;
            while ((line = reader.readLine()) != null) {
                if (line.contains("Publics by Value")) {
                    inPublics = true;
                    continue;
                }
                if (!inPublics) continue;

                Matcher m = pattern.matcher(line);
                if (m.find()) {
                    symbols.add(new String[]{ m.group(1), m.group(2) });
                }
            }
        }

        println("Parsed " + symbols.size() + " symbols from map file");

        FunctionManager fm = currentProgram.getFunctionManager();
        Memory memory = currentProgram.getMemory();

        int applied = 0;
        int renamed = 0;
        int skipped = 0;

        for (String[] entry : symbols) {
            if (monitor.isCancelled()) break;

            String symbolName = entry[0];
            long addrLong = Long.parseUnsignedLong(entry[1], 16);
            Address addr = currentProgram.getAddressFactory()
                .getDefaultAddressSpace().getAddress(addrLong);

            if (addr == null || !memory.contains(addr)) {
                skipped++;
                continue;
            }

            Function func = fm.getFunctionAt(addr);
            if (func != null) {
                String funcName = func.getName();
                boolean isAuto = funcName.startsWith("FUN_")
                    || funcName.startsWith("Function_")
                    || funcName.startsWith("thunk_FUN_");

                if (isAuto) {
                    // Remove duplicate labels first
                    for (Symbol existing : currentProgram.getSymbolTable().getSymbols(addr)) {
                        if (existing.getName().equals(symbolName)
                            && !existing.equals(func.getSymbol())) {
                            existing.delete();
                        }
                    }
                    try {
                        func.setName(symbolName, SourceType.IMPORTED);
                        renamed++;
                    } catch (Exception e) {
                        // Fall back to label
                        SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                            currentProgram, addr, null, symbolName, SourceType.IMPORTED);
                    }
                } else {
                    SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                        currentProgram, addr, null, symbolName, SourceType.IMPORTED);
                }
            } else {
                SymbolUtilities.createPreferredLabelOrFunctionSymbol(
                    currentProgram, addr, null, symbolName, SourceType.IMPORTED);
            }
            applied++;
        }

        println("Applied " + applied + " symbols, renamed " + renamed
            + " functions (" + skipped + " skipped)");
    }
}
