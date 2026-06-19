# bonhomme demo

React/Vite dashboard for the local bonhomme Rust API.

Start the API from the repository root:

```sh
export BONHOMME_TSC="$PWD/demo/node_modules/.bin/tsc"
cargo run -p bonhomme -- server
```

Then run the demo:

```sh
npm install
npm run dev
```

The dashboard can reset the demo repository, create many ready agent branches, merge them live, or run the backend stress simulation.
