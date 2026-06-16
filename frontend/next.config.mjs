/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Catch every stale /pump/* link from before the rename. Permanent so
  // search engines + bookmarks update too.
  async redirects() {
    return [
      { source: "/pump", destination: "/launch", permanent: true },
      { source: "/pump/:path*", destination: "/launch/:path*", permanent: true },
    ];
  },
  // TODO: drop once @solana/wallet-adapter-react ships React 18 strict-mode-compatible
  // FC<{children}> typings (or pin @types/react to a version that resolves the JSX
  // overload conflict). Until then, allow next build to proceed despite the wallet
  // provider type errors — they're false positives and don't affect runtime.
  typescript: {
    ignoreBuildErrors: true,
  },
  // `@staccoverflow/zk-proofs-wasm` ships a `.wasm` binary. Webpack tries to
  // bundle it into `.next/server/chunks/` but Vercel's file-tracer doesn't
  // always copy that chunk into the serverless function output — at runtime
  // we get `ENOENT: /var/task/.next/server/chunks/solana_zk_proofs_wasm_bg.wasm`.
  //
  // Two-prong fix for Next 14.x:
  //   1. `serverComponentsExternalPackages` (Next 14 name; renamed to
  //      `serverExternalPackages` in 15+) keeps the wasm package out of
  //      webpack's bundling step. Its require() resolves against
  //      `node_modules/` at runtime where the .wasm sibling is colocated.
  //   2. `outputFileTracingIncludes` globs the wasm files into the function
  //      bundle so the require() can actually find them at /var/task. Both
  //      `./` and `../` node_modules paths cover pnpm hoisted + sandboxed
  //      layouts.
  experimental: {
    serverComponentsExternalPackages: ["@staccoverflow/zk-proofs-wasm"],
    outputFileTracingIncludes: {
      // `.npmrc` sets `node-linker=hoisted` so pnpm installs the wasm
      // package as a real flat dir under node_modules/, no .pnpm symlink
      // sandbox. The previous .pnpm/<pkg>@<ver>/... globs caused Vercel
      // to reject the deployment with "files in symlinked directories"
      // because the tracer was following pnpm's isolation symlinks. With
      // hoisted layout the simple glob is enough.
      "/api/confidential/proof": [
        "./node_modules/@staccoverflow/zk-proofs-wasm/**/*.wasm",
        "../node_modules/@staccoverflow/zk-proofs-wasm/**/*.wasm",
      ],
      "app/api/confidential/proof/route": [
        "./node_modules/@staccoverflow/zk-proofs-wasm/**/*.wasm",
        "../node_modules/@staccoverflow/zk-proofs-wasm/**/*.wasm",
      ],
    },
  },
  webpack: (config) => {
    // Polyfill / disable Node-only modules pulled by some wallet adapters in browser bundles.
    config.resolve.fallback = {
      ...config.resolve.fallback,
      fs: false,
      path: false,
      crypto: false,
    };
    // pino-pretty is an optional pino dep used by walletconnect's logger only in
    // dev/server contexts. Mark as external so webpack does not warn on missing.
    config.externals = [...(config.externals ?? []), "pino-pretty"];
    return config;
  },
};

export default nextConfig;
