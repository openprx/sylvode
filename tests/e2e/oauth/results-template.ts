// Markdown template of the acceptance record. `renderMarkdown` fills the {{placeholders}}.

export const RESULTS_TEMPLATE = `# Sylvode P4 acceptance record (WP8)

Result: **{{result}}**

| | |
|---|---|
| Started | {{started}} |
| Finished | {{finished}} |
| UI origin (issuer) | {{ui}} |
| API origin | {{api}} |
| MCP origin | {{mcp}} |
| Build commit of the environment | {{commit}} ({{commit_source}}) |
| Cases | {{summary}} |
| Ramflux client at the end of the run | {{client_state}} |

## Timing figures

The limit is 5 s for each figure. Intervals are measured with a monotonic clock in the client from the
trigger named in the table to the moment the chunk carrying the message arrived.

{{timings_table}}

## Cases

{{cases_table}}

## Failures

{{failures}}

## Artifact self-check

{{scan}}

## Notes

- The per-case files under \`cases/\` hold the details of each case; a failed case also holds the raw
  requests and responses (status, relevant headers, body) with every token, code, verifier and
  password replaced.
- No token, code, verifier or password is written to this directory. Connections are identified by the
  grant id that \`GET /api/v1/oauth/connection\` returns.
- The same record in machine form is \`acceptance.json\`.
`;
