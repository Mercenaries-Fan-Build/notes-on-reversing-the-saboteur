// Disassembly export of chosen functions. Arguments: the output file, then one or more function names
// (exact symbol names, as the decomp export's "==== <name> @..." headers print them) or entry addresses
// (0x-prefixed). Each function is written as
//   ==== <name> @0xADDR ====
// followed by one line per instruction: address, bytes, mnemonic and operands, and the names of the
// functions or labels its references point at. A name or address that matches no function is an error.
//
// @category Saboteur
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;

import java.io.File;
import java.io.PrintWriter;
import java.util.ArrayList;
import java.util.List;

public class ExportListing extends GhidraScript {
    private Function resolve(String arg) throws Exception {
        if (arg.startsWith("0x")) {
            Address a = toAddr(Long.parseUnsignedLong(arg.substring(2), 16));
            Function f = getFunctionAt(a);
            if (f == null) {
                throw new IllegalArgumentException("no function at " + arg);
            }
            return f;
        }
        List<Function> hits = new ArrayList<>();
        SymbolIterator it = currentProgram.getSymbolTable().getSymbols(arg);
        while (it.hasNext()) {
            Symbol s = it.next();
            Function f = getFunctionAt(s.getAddress());
            if (f != null && !hits.contains(f)) {
                hits.add(f);
            }
        }
        if (hits.size() != 1) {
            throw new IllegalArgumentException(arg + " names " + hits.size() + " functions; pass an address");
        }
        return hits.get(0);
    }

    @Override
    public void run() throws Exception {
        String[] argv = getScriptArgs();
        if (argv.length < 2) {
            throw new IllegalArgumentException(
                "ExportListing.java needs the output file and at least one function name or address");
        }
        List<Function> funcs = new ArrayList<>();
        for (int i = 1; i < argv.length; i++) {
            funcs.add(resolve(argv[i]));
        }
        try (PrintWriter fp = new PrintWriter(new File(argv[0]), "UTF-8")) {
            for (Function f : funcs) {
                fp.println(String.format("==== %s @0x%08x ====", f.getName(), f.getEntryPoint().getOffset()));
                InstructionIterator ins = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (ins.hasNext()) {
                    Instruction in = ins.next();
                    Symbol label = getSymbolAt(in.getAddress());
                    if (label != null && !in.getAddress().equals(f.getEntryPoint())) {
                        fp.println(label.getName() + ":");
                    }
                    StringBuilder bytes = new StringBuilder();
                    for (byte b : in.getBytes()) {
                        bytes.append(String.format("%02x", b & 0xff));
                    }
                    StringBuilder refs = new StringBuilder();
                    for (Reference r : in.getReferencesFrom()) {
                        Symbol s = getSymbolAt(r.getToAddress());
                        if (s != null) {
                            refs.append(refs.length() == 0 ? "  ; " : ", ").append(s.getName());
                        }
                    }
                    fp.println(String.format("  %08x  %s  %s%s", in.getAddress().getOffset(), bytes, in, refs));
                }
                fp.println();
            }
        }
        println("wrote " + funcs.size() + " functions to " + argv[0]);
    }
}
