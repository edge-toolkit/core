// WebAssembly streaming: deno_fetch Responses aren't recognised by V8's
// native streaming compile/instantiate, so fall back to arrayBuffer +
// instantiate/compile. Without this, the dotnet loader's
// `compileStreaming(fetch(...))` path throws.
//
// Loaded after main.js by `shim_js()` in runtime.rs.
{
  WebAssembly.instantiateStreaming = async (source, imports) => {
    const resp = await source;
    const bytes = await resp.arrayBuffer();
    return WebAssembly.instantiate(bytes, imports);
  };
  WebAssembly.compileStreaming = async (source) => {
    const resp = await source;
    const bytes = await resp.arrayBuffer();
    return WebAssembly.compile(bytes);
  };
}
