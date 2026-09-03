# Package scope

Packages are published under the account-controlled `@writerslogic` npm scope.
The public entry point is `@writerslogic/audio-provenance-sdk`; the remaining
packages and Rust crates are internal implementation components.

To change it: update the `name` field in every `packages/*/package.json`, the
matching workspace dependency specifiers, and the four places a published
package names itself — `packages/sdk/README.md`, `packages/sdk/examples/*.js`,
`packages/sdk/test/*.js` and the root `package.json` scripts, all of which import
by package name so that what ships is what the tests actually exercised. Nothing
else encodes the npm scope; no Rust crate does.
