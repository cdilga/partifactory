# The part CLI

The `part` command is the partifactory binary invoked through a `part` symlink. It calls the same operation registry as the HTTPS API.

```sh
export PARTIFACTORY_URL=https://parts.example.com
export PARTIFACTORY_TOKEN='<personal-token>'
part put ./dist --name packing --kind files
part get packing
part edit packing --replace 'passports' 'passports and chargers'
part part_get --json @request.json
part part_edit --json - < edit.json
part help part_edit
```

`put` uploads only missing content hashes. When the name already exists, it compares current file hashes and submits one atomic edit batch. Unchanged files upload nothing. A request retries once after a connection error or timeout, with the same arguments and request id. The CLI never follows redirects with credentials.

`PARTIFACTORY_URL` chooses the server. Tokens are read from `PARTIFACTORY_TOKEN`, then the macOS keychain (`partifactory`, account = server URL), then a JSON config file. The default config is `$XDG_CONFIG_HOME/partifactory/config.json` or `~/.config/partifactory/config.json`; `PARTIFACTORY_CONFIG` overrides the path. On Unix the file must have permissions `0600`.

```json
{"url":"https://parts.example.com","token":"<personal-token>"}
```

`PARTIFACTORY_CA_CERT` adds a PEM certificate authority for a private HTTPS deployment or the e2e harness. Certificate and hostname verification remain enabled. HTTP is accepted only for a loopback development server.

Successful results are JSON on stdout. Errors are JSON on stderr and include the server's request id. Exit codes follow PLAN §4b: 2 invalid input; 3 authentication or permission; 4 missing; 5 conflict or anchor; 6 quota or rate limit; 7 unavailable, storage or internal error.
