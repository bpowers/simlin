# engine2

A C-compatible FFI interface to the Simlin system dynamics simulation engine, providing language-agnostic access to simulation capabilities through WebAssembly.

## Purpose

engine2 exposes the core Simlin simulation engine through a stable C ABI, enabling integration with any programming language that can call C functions or execute WebAssembly modules. This allows system dynamics models to be simulated in environments beyond the web browser, including server-side applications, scientific computing environments, and embedded systems.

## Architecture

The library is structured in three layers:

1. **Rust FFI Layer** (`src/lib.rs`): Wraps the simlin-engine with C-compatible exports
2. **WebAssembly Module** (`engine2.wasm`): Compiled for the `wasm32-wasip1` target
3. **Language Bindings**: Currently Go, with potential for Python, C++, and others

## Core Capabilities

### Project Management
- Load system dynamics projects from protobuf format
- Reference counting for safe memory management across FFI boundaries
- Support for multi-model projects with module interconnections

### Simulation Control
- Step-by-step or full simulation execution
- Real-time variable value access and modification during simulation
- Configurable time bounds and step sizes
- Reset simulations to initial conditions

### Data Access
- Query variable names and counts
- Get/set values by variable name or offset
- Retrieve complete time series data for any variable
- Access simulation metadata (step count, current time)

### Loop Analysis (LTM)
- Detect and analyze feedback loops in models
- Classify loops as reinforcing or balancing
- Calculate relative loop strength scores
- Identify variables participating in each loop

## API Overview

The C API exposes approximately 25 functions with the `simlin_` prefix:

### Project Operations
- `simlin_project_open`: Load a project from protobuf bytes
- `simlin_project_ref/unref`: Reference counting
- `simlin_project_enable_ltm`: Enable Loop Thinking Method analysis

### Simulation Operations
- `simlin_sim_new`: Create a simulation from a project
- `simlin_sim_run_to`: Run simulation to a specific time
- `simlin_sim_run_to_end`: Complete the simulation
- `simlin_sim_reset`: Reset to initial conditions

### Data Operations
- `simlin_sim_get_value`: Get current variable value
- `simlin_sim_set_value`: Modify variable value
- `simlin_sim_get_series`: Retrieve complete time series
- `simlin_sim_get_varnames`: List all variables

### Analysis Operations
- `simlin_analyze_get_loops`: Detect feedback loops
- `simlin_analyze_get_rel_loop_score`: Calculate loop importance

### Memory Management
- `simlin_malloc/free`: WebAssembly memory allocation
- `simlin_free_string`: Free C strings returned by API
- `simlin_free_loops`: Clean up loop analysis results

## Error Handling

All functions that can fail return an error code corresponding to the simlin-engine `ErrorCode` enum. Use `simlin_error_str` to get human-readable error descriptions. Common error codes include:

- `NoError` (0): Success
- `DoesNotExist`: Variable or model not found
- `CircularDependency`: Model contains algebraic loops
- `NotSimulatable`: Model cannot be simulated in current state
- `BadSimSpecs`: Invalid simulation parameters

## Building

### WebAssembly Module
```bash
./build.sh  # Builds engine2.wasm using wasm32-wasip1 target
```

### Go Bindings
```bash
go test  # Run tests using the Wazero runtime
```

## Usage Example (Go)

```go
// Load a project
projectBytes, _ := os.ReadFile("model.pb")
project, err := OpenProject(projectBytes)
if err != nil {
    log.Fatal(err)
}
defer project.Close()

// Create and run simulation
sim, err := project.NewSim("main")
if err != nil {
    log.Fatal(err)
}
defer sim.Close()

err = sim.RunToEnd()
if err != nil {
    log.Fatal(err)
}

// Access results
value, err := sim.GetValue("population")
series, err := sim.GetSeries("infection_rate")
```

## Memory Safety

The library uses reference counting to ensure memory safety across the FFI boundary. Always:
- Call `unref` functions when done with objects
- Free strings returned by the API using `simlin_free_string`
- Free loop analysis results using `simlin_free_loops`

## Go Bindings Concurrency Pattern

The Go bindings use a mutex to serialize access to the WebAssembly runtime. To avoid deadlocks, functions that need to call other mutex-protected functions use a locked/unlocked pattern:

- Public methods acquire the mutex and call internal `*Locked` methods
- Internal `*Locked` methods assume the caller holds the mutex
- This prevents deadlock when error handling or other internal operations need to call protected functions

Example: `GetErrorString` acquires the lock and calls `getErrorStringLocked`, which can be safely called from within other mutex-protected functions.

## Platform Support

The WebAssembly module targets `wasm32-wasip1` for maximum compatibility:
- WASI support provides file system and time access
- No browser-specific dependencies
- Works with any WASM runtime supporting WASI (Wasmtime, Wazero, etc.)

## Testing

Test data and examples are provided in `testdata/`:
- `SIR_project.pb`: Example epidemiological model
- `SIR_output.csv`: Expected simulation results

Run tests with: `cargo test -p engine2` (Rust) or `go test` (Go bindings)