import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { DEV_ID_PATH, DEV_ID_PREFIX, DEV_PORT } from "./dev-server.config";

export default defineConfig({
  plugins: [
    react(),
    {
      name: "rrss-dev-id",
      apply: "serve",
      configureServer(server) {
        // 别的检出的身份路径回 404，不能落进 SPA 回退的 200
        server.middlewares.use((req, res, next) => {
          if (!req.url?.startsWith(DEV_ID_PREFIX)) return next();
          res.statusCode = req.url === DEV_ID_PATH ? 200 : 404;
          res.end();
        });
      },
    },
    {
      name: "classic-script",
      apply: "build",
      transformIndexHtml: {
        order: "post",
        handler(html: string) {
          let out = html.replace(/\s+crossorigin(?:="[^"]*")?/g, "").replace(/\s+type="module"/g, "");
          out = out.replace(/<script[^>]*src="[^"]*index\.js"[^>]*><\/script>\s*/g, "");
          return out.replace("</body>", `    <script defer src="./assets/index.js"></script>\n  </body>`);
        },
      },
    },
  ],
  // Tauri 自定义协议吃相对路径；`/` 在 Linux `tauri://localhost` 上会打到空页。
  base: "./",
  clearScreen: false,
  server: {
    port: DEV_PORT,
    strictPort: true,
    host: "127.0.0.1",
    hmr: {
      host: "127.0.0.1",
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "safari15",
    cssCodeSplit: false,
    modulePreload: false,
    rollupOptions: {
      output: {
        format: "iife",
        inlineDynamicImports: true,
        entryFileNames: "assets/index.js",
        assetFileNames: "assets/[name][extname]",
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    // CI 与其他任务同机抢核时，挂整个首页再等几轮 flush 的用例可能超过 5 秒；本地保持 5 秒，尽早暴露真慢的用例。
    testTimeout: process.env.CI ? 20000 : 5000,
  },
});
