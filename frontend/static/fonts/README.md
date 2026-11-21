# Self-hosted fonts

Variable (wght) WOFF2 subsets taken unchanged from the Fontsource packages below.
Both faces are licensed under the SIL Open Font License 1.1 (full text in the LICENSE files here).

| Face | npm package | Version | Licence | Subsets kept |
|---|---|---|---|---|
| Hanken Grotesk | `@fontsource-variable/hanken-grotesk` | 5.3.0 | OFL-1.1 | latin, latin-ext, vietnamese |
| JetBrains Mono | `@fontsource-variable/jetbrains-mono` | 5.3.0 | OFL-1.1 | latin, latin-ext |

The `@font-face` rules are in `src/app.css`. They declare the families `Hanken Grotesk` and
`JetBrains Mono`, the names used by the `--rf-font-*-stack` tokens in `src/tokens.css`.
Nothing applies them yet.
