default:
    @just --list

dev:
    cargo run -- ../the-last-stag-mud/hearth.toml

web-dev:
    cd web && npm run dev

web-build:
    cd web && npm install && npm run build

test:
    cargo test

# Cut a release: stamp the changelog, bump, test, commit, tag, push.
# `just release` takes the next rc; pass a version for anything else
# (`just release 0.1.0`). `PUSH=0 just release` stops before pushing.
release version="":
    ./scripts/release.sh "{{version}}"

# Release gate on its own — no side effects. Checks the tag, Cargo.toml, and the
# CHANGELOG section agree, then runs the suite. The Release workflow runs the
# same script on the pushed tag, so a green run here means the tag will publish.
release-check version:
    ./scripts/check-release.sh "{{version}}"
    cargo test

bundle:
    just web-build
    cargo build --release --features bundle-web

install dest="~/.local/bin":
    just bundle
    install -m 755 target/release/hearth-mud "{{dest}}/"

clean:
    cargo clean
    rm -rf web/dist
