# @simlin/mcp

MCP (Model Context Protocol) server for [Simlin](https://simlin.com), a system dynamics modeling tool. This server lets AI assistants read, create, and edit stock-and-flow simulation models.

## Setup

### Claude Code

```sh
claude mcp add simlin npx @simlin/mcp@latest
```

### Claude Desktop

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows):

```json
{
  "mcpServers": {
    "simlin": {
      "command": "npx",
      "args": ["@simlin/mcp@latest"]
    }
  }
}
```

### Other MCP clients

Any MCP-compatible client can run the server over stdio:

```sh
npx @simlin/mcp@latest
```

## Tools

| Tool | Description |
|------|-------------|
| **ReadModel** | Read a model file and return a JSON snapshot with loop dominance analysis |
| **EditModel** | Apply operations (upsert/remove variables, name loops) to an existing model |
| **CreateModel** | Create a new empty model file |

### Supported file formats

| Format                  | Extensions                | Read | Edit |
|-------------------------|---------------------------|------|------|
| XMILE                   | `.stmx`, `.xmile`, `.xml` | Yes | Yes |
| JSON (Simlin and SD-AI) | `.sd.json`, `.json`       | Yes | Yes |
| Vensim                  | `.mdl`                    | Yes | No (import only) |

## Skill resources

The PyPI package is `pysimlin`:

```sh
pip install pysimlin
```

Imported in Python as `simlin`:

```python
import simlin

model = simlin.load("population.stmx")
run = model.run()
print(run.results["population"].iloc[-1])
```

The server also exposes skill resources around how to use the Python library:

- `simlin://skills/pysimlin-basics` -- Loading models, simulation, DataFrame access
- `simlin://skills/scenario-analysis` -- Parameter sweeps and intervention analysis
- `simlin://skills/loop-dominance` -- Feedback loop analysis and visualization
- `simlin://skills/vensim-equation-syntax` -- Vensim-to-XMILE function mapping

## Verifying the build

After making changes to `simlin-mcp` or `simlin-mcp-core`, exercise the rebuilt binary against a real MCP host before publishing. The smoke test below uses the official [MCP Inspector](https://github.com/modelcontextprotocol/inspector) and a fixture from this repository's `test/test-models/`.

1. Build the release binary:

    ```sh
    cargo build -p simlin-mcp --release
    ```

2. Launch Inspector against the binary. It opens a browser tab speaking to the server over stdio:

    ```sh
    npx @modelcontextprotocol/inspector ./target/release/simlin-mcp
    ```

3. In Inspector's left pane, click **Connect**. Verify the `initialize` exchange shows:

    - `protocolVersion: "2025-11-25"`
    - `serverInfo.name: "simlin-mcp"`
    - `capabilities.tools` and `capabilities.resources` both present

4. Switch to the **Tools** tab and click **List Tools**. You should see exactly three entries: `read_model`, `edit_model`, `create_model`.

5. Select `read_model` and call it with the absolute path to the teacup fixture, e.g.:

    ```json
    { "project_path": "/absolute/path/to/simlin/test/test-models/samples/teacup/teacup.xmile" }
    ```

    The response should include both a `content` array (one text item with the JSON snapshot) and a `structuredContent` field carrying the same payload as a JSON object. Tools that return validation errors set `isError: true` while still populating `structuredContent`.

6. Switch to the **Resources** tab and click **List Resources**. Verify all four URIs appear:

    - `simlin://skills/pysimlin-basics`
    - `simlin://skills/scenario-analysis`
    - `simlin://skills/loop-dominance`
    - `simlin://skills/vensim-equation-syntax`

    Click each entry and confirm the body is non-empty markdown. The `pysimlin-basics` resource must contain the version from `pysimlin.version` -- if it still shows `{PYSIMLIN_VERSION}`, `build.rs` did not run.

If any step fails, the failure points to a specific layer: step 3 means rmcp's stdio handshake is broken, step 4 means the `#[tool_router]` registration in `simlin-mcp-core` is wrong, step 5 means a tool's wire shape regressed, and step 6 means the resources `Vec` passed to `SimlinMcpServer::new` was built incorrectly.

## License

Apache-2.0
