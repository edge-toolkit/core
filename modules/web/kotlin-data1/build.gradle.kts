plugins {
    kotlin("multiplatform") version "2.4.10"
}

repositories {
    mavenCentral()
}

kotlin {
    wasmJs {
        // The compiled loader + wasm artifact names derive from this; the committed pkg/ shim imports
        // `et_ws_kotlin_data1_compiled.mjs`, mirroring dart-data1's `*_compiled.js` naming.
        outputModuleName.set("et_ws_kotlin_data1_compiled")
        browser()
        binaries.executable()
    }
    compilerOptions {
        allWarningsAsErrors.set(true)
    }
}

// Copy the linked production executable (WasmGC module + its ES-module loader glue) into pkg/, where the
// modules service serves it next to the committed package.json and shim. `preserve` keeps the committed files.
tasks.register<Sync>("pkgDist") {
    dependsOn("wasmJsProductionExecutableCompileSync")
    from(layout.buildDirectory.dir("compileSync/wasmJs/main/productionExecutable/kotlin"))
    into(layout.projectDirectory.dir("pkg"))
    preserve {
        include("package.json", "et_ws_kotlin_data1.js", ".gitignore")
    }
}
