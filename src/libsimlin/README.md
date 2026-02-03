# libsimlin

A C-compatible FFI interface to the Simlin system dynamics simulation engine, providing language-agnostic access to simulation capabilities through WebAssembly.


## Overview

libsimlin exposes the core Simlin simulation engine through a stable C ABI, enabling integration with any programming language that can call C functions or execute WebAssembly modules.
This allows system dynamics models to be simulated in environments beyond the web browser, including server-side applications written in languages other than Rust.


## Architecture

The library exposes one major component:

1. **Rust FFI Layer** (`src/lib.rs`): Wraps the simlin-engine with C-compatible exports


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

The C API exposes approximately 25 functions with the `simlin_` prefix, see [simlin.h](./simlin.h) for details.
Some major components are: 

### Project Operations
- `simlin_project_open_protobuf`: Load a project from protobuf bytes
- `simlin_project_open_json`: Load a project from JSON
- `simlin_project_open_xmile`: Load a project from XMILE/STMX
- `simlin_project_open_vensim`: Load a project from Vensim MDL
- `simlin_project_serialize_protobuf`: Serialize a project to protobuf bytes
- `simlin_project_serialize_json`: Serialize a project to JSON
- `simlin_project_serialize_xmile`: Serialize a project to XMILE/STMX
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


## Memory Safety

The library uses reference counting to ensure memory safety across the FFI boundary. Always:
- Call `unref` functions when done with objects
- Free strings returned by the API using `simlin_free_string`
- Free loop analysis results using `simlin_free_loops`


### Variable Name Resolution

The engine uses canonicalized names internally (lowercase, spaces → underscores). The FFI layer handles canonicalization automatically, but be aware that:
- `"Infectious"` and `"infectious"` resolve to the same variable (variable names are case-insensitive)
- The API tries exact matches first, then suffix matches for module-qualified names
- Use `simlin_sim_get_varnames()` to see the exact names the engine recognizes
