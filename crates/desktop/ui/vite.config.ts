import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { HTML_CSP, isolatedHtml, parseHtmlDocument } from "./src/lib/htmlArtifacts.ts";

const root = path.dirname(fileURLToPath(import.meta.url));

function htmlFixtureEndpoint(): Plugin {
  const views = new Map<string, { html: string; expires: number }>();
  return {
    name: "zest-offline-html-fixture",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        const pathname = request.url?.split("?")[0] ?? "";
        if (pathname !== "/__zest_html" && !pathname.startsWith("/__zest_html/")) return next();
        response.setHeader("Cache-Control", "no-store");
        response.setHeader("X-Content-Type-Options", "nosniff");
        for (const [token, view] of views) if (view.expires <= Date.now()) views.delete(token);
        const origin = `http://${request.headers.host}`;
        const trustedHost = /^127\.0\.0\.1:\d+$/.test(request.headers.host ?? "");
        const mutation = request.method === "POST" || request.method === "DELETE";
        if (!trustedHost || (mutation && request.headers.origin !== origin)) {
          response.statusCode = 403;
          response.end("HTML fixture requests must come from the main app.");
          return;
        }
        try {
          if (request.method === "POST" && pathname === "/__zest_html") {
            if (!request.headers["content-type"]?.startsWith("application/json")) throw new Error("Expected JSON.");
            const chunks: Buffer[] = [];
            let bytes = 0;
            for await (const chunk of request) {
              const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
              bytes += buffer.length;
              if (bytes > 4 * 1024 * 1024) throw new Error("HTML request is too large.");
              chunks.push(buffer);
            }
            const input: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
            const html = isolatedHtml(parseHtmlDocument(input));
            if (views.size >= 64) throw new Error("Too many HTML views. Close a preview and try again.");
            const token = randomUUID();
            views.set(token, { html, expires: Date.now() + 5 * 60_000 });
            response.setHeader("Content-Type", "application/json");
            response.end(JSON.stringify({ token, url: `${origin}/__zest_html/${token}` }));
            return;
          }
          const route = /^\/__zest_html\/([a-f0-9-]+)(\/document)?$/.exec(pathname);
          if (route && request.method === "DELETE" && !route[2]) {
            views.delete(route[1]);
            response.statusCode = 204;
            response.end();
            return;
          }
          const view = route ? views.get(route[1]) : undefined;
          if (request.method !== "GET" || !view) {
            response.statusCode = 404;
            response.end("HTML view is missing or expired.");
            return;
          }
          response.setHeader("Content-Type", "text/html;charset=utf-8");
          if (route?.[2]) {
            response.setHeader("Content-Security-Policy", HTML_CSP);
            response.end(view.html);
          } else {
            // The exact child route also confines location changes inside the artifact.
            const documentUrl = `${origin}${pathname}/document`;
            response.setHeader("Content-Security-Policy", `sandbox allow-scripts; default-src 'none'; frame-src ${documentUrl}; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'`);
            response.end(`<!doctype html><meta charset="utf-8"><title>Offline HTML viewer</title><style>html,body{height:100%;margin:0}iframe{display:block;width:100%;height:100%;border:0}</style><iframe title="HTML document" sandbox="allow-scripts" referrerpolicy="no-referrer" src="${documentUrl}"></iframe>`);
          }
        } catch (error) {
          response.statusCode = 400;
          response.setHeader("Content-Type", "text/plain;charset=utf-8");
          response.end(error instanceof Error ? error.message : "Invalid HTML request.");
        }
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), htmlFixtureEndpoint()],
  // Workbench loads lazily; prepare its avatar entry before the first click.
  optimizeDeps: {
    include: ["blobatar/react"],
  },
  resolve: {
    alias: {
      "@": path.resolve(root, "./src"),
    },
  },
  // Emit the Shiki worker as a real ES module file rather than a blob, so the
  // production CSP can stay at `worker-src 'self'` without allowing `blob:`.
  worker: {
    format: "es",
  },
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
});
