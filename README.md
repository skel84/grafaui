# Grafaui

[![macOS](https://github.com/skel84/grafaui/actions/workflows/macos.yml/badge.svg)](https://github.com/skel84/grafaui/actions/workflows/macos.yml)

Grafana dashboard JSON rendered as a native macOS app with Rust and
[GPUI Kit](https://gpui-kit.com). Use deterministic fake data to explore panels,
or connect to one Prometheus endpoint for live instant and range queries.

Click the dashboard title to search the local collection by title, filename or
tag. The repository includes 98 dashboard examples. Switching keeps the data
mode and Prometheus connection; failed real requests show errors.

## Download

Open a successful run in [GitHub Actions](https://github.com/skel84/grafaui/actions/workflows/macos.yml)
and download the artifact for your Mac:

- `grafaui-aarch64-apple-darwin`: Apple Silicon (arm64).
- `grafaui-x86_64-apple-darwin`: Intel (x86_64).

Unzip the artifact, then verify and extract its archive:

```sh
shasum -a 256 -c grafaui-aarch64-apple-darwin.tar.gz.sha256
tar -xzf grafaui-aarch64-apple-darwin.tar.gz
cd grafaui-aarch64-apple-darwin
./grafaui --fake
```

Substitute `x86_64` for `aarch64` on Intel. Archives contain a command-line
executable, dashboard JSON and documentation. Keep `fixtures` beside the binary
to retain the complete dashboard picker. Builds are unsigned and are tested on
macOS 15; signing, notarization and app bundles are not configured.

## Run

```sh
./grafaui --fake
./grafaui --fixture node-exporter --prometheus-url http://127.0.0.1:9090 --scrape-interval 30
./grafaui /path/to/dashboard.json --dashboard-dir /path/to/dashboards --fake
```

`--dashboard-dir` recursively indexes a custom JSON collection instead of the
included fixtures. The three built-in examples remain available. The catalog
is scanned at startup; restart to discover new files or changed titles/tags.

For a Prometheus service in Kubernetes, forward it in a separate terminal:

```sh
kubectl --kubeconfig /path/to/kubeconfig -n NAMESPACE \
  port-forward --address 127.0.0.1 svc/PROMETHEUS_SERVICE 9090:9090
```

Set `--scrape-interval` to the endpoint's actual scrape interval (seconds).
Prometheus mode supports relative dashboard ranges ending at `now`, dependent
variables, common Grafana macros, target intervals and legend templates. Every
Prometheus datasource name/UID uses the configured endpoint. Authentication,
multiple endpoints, native histograms and repeat expansion are not supported.
See [verification and limitations](fixtures/reports/prometheus-connection.txt).
Successful empty variable queries display a warning and allow panel requests to
continue. See the [live variable investigation](fixtures/reports/prometheus-variable-investigation.txt)
for dashboard compatibility checks and remaining fixture-specific limitations.

## Build and test

Install Rust stable and full Xcode with its Metal compiler, then run:

```sh
cargo test --workspace --release --locked
cargo build --release --locked
./target/release/grafaui --fake
```

The macOS workflow runs formatting, Clippy, snapshot verification, release tests
and a release build on native Intel and Apple Silicon runners. It runs on pushes
to `main`, pull requests, `v*` tags and manual dispatch. Each architecture uploads
an archive and checksum. Dependencies are locked and Rust build outputs cached.

Crates separate pure dashboard parsing/data models (`grafaui-model`), Prometheus
queries (`grafaui-prometheus`) and native rendering (`grafaui-desktop`). The query
crate plans every call as data, variables included (`plan::VariablePlan`), so
another transport can send it as a GET or a POST; its blocking HTTP `Client` sits
behind the default `client` feature.

## License

Grafaui source is MIT licensed. Imported dashboards retain their upstream
licenses; [fixture provenance](fixtures/sources/expanded.json) records source
URLs, revisions, hashes and licenses for the expanded snapshots. The MIT license
does not relicense third-party dashboards or dependencies.
