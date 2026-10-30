# Connect an MCP client

Import an archive first using the [CLI](../README.md#import).

Runs an MCP server exposing `search_tweets`, `search_likes`, `query` and
`schema` as tools, so MCP clients such as Claude Code can search the
archive. The database is opened read-only.

## Connect over stdio

```sh
twarq mcp --db archive.duckdb
```

Configure the client to start `twarq` over stdio:

```json
{
  "mcpServers": {
    "twarq": {
      "command": "twarq",
      "args": ["mcp", "--db", "/path/to/archive.duckdb"]
    }
  }
}
```

## Connect over Streamable HTTP

```sh
twarq mcp --db archive.duckdb --http 127.0.0.1:8080
```

With `--http` the same tools are served over Streamable HTTP at
`http://<addr>/mcp` (`"url"` instead of `"command"` in the client config).
The HTTP mode is stateless — no session handling — and only accepts
requests whose `Host` header is a loopback address, which is what protects
a locally running server from DNS rebinding. Put it behind a reverse proxy
or a tunnel if you need to reach it from another machine.
