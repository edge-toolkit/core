// The probe page's script for a browser with neither ES modules nor WebAssembly, such as Safari 9 on an iPad 3: what
// such a browser lacks first, then the page's own script, which tsconfig.json beside this file points at the probes
// as JavaScript.
// skipcq: JS-0833 -- committed ES module; the analyzer's script-mode parse is a false positive
import "./polyfills.js";
import "/probe.js";
