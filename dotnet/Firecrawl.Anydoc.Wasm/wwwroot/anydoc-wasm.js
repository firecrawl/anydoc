// Firecrawl.Anydoc.Wasm interop shim.
//
// Loaded once from managed code via Blazor's `import` helper. It lazy-loads
// the wasm-bindgen glue produced from the anydoc `wasm/` crate, instantiates
// the engine, and re-exposes the conversion entry points as plain functions
// that the C# AnydocConverter calls through IJSRuntime.
//
// Conversion errors thrown by the engine (a JS Error with a `code` property)
// are re-thrown with a parseable `anydoc:<code>:<message>` message so the
// managed side can reconstruct an AnydocException with the right Kind.

let gluePromise = null;

function glue() {
    if (!gluePromise) {
        gluePromise = (async () => {
            const m = await import("./anydoc_wasm.js");
            // The wasm binary sits next to this shim; resolve it against
            // import.meta.url, not the app's base path.
            const wasmUrl = new URL("./anydoc_wasm_bg.wasm", import.meta.url);
            await m.default(wasmUrl);
            return m;
        })();
    }
    return gluePromise;
}

function rethrowConversion(error) {
    const code = (error && error.code) || "unknown";
    const message = (error && error.message) || String(error);
    throw new Error(`anydoc:${code}:${message}`);
}

export async function formatFromBytes(bytes) {
    const m = await glue();
    return m.formatFromBytes(bytes);
}

export async function formatFromExtension(extension) {
    const m = await glue();
    return m.formatFromExtension(extension);
}

export async function formatFromPath(path) {
    const m = await glue();
    return m.formatFromPath(path);
}

export async function toMarkdownBytes(bytes, format) {
    const m = await glue();
    try {
        return m.toMarkdownBytes(bytes, format || undefined);
    } catch (error) {
        throw rethrowConversion(error);
    }
}

export async function toDocument(bytes, format) {
    const m = await glue();
    try {
        return m.toDocument(bytes, format || undefined);
    } catch (error) {
        throw rethrowConversion(error);
    }
}