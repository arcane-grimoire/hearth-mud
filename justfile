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

bundle:
    just web-build
    cargo build --release --features bundle-web

install dest="~/.local/bin":
    just bundle
    install -m 755 target/release/hearth-mud "{{dest}}/"

clean:
    cargo clean
    rm -rf web/dist
