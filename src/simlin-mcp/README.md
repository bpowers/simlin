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

## License

Apache-2.0
