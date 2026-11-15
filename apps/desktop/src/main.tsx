import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyLanguage, resolveLanguage } from "./i18n";
import { api } from "./lib/api";
import { startSignals } from "./lib/signals";
import { installWheelContainment } from "./lib/wheelContainment";
import { startClientLog } from "./lib/clientLog";
import "./styles/app.css";

// 错误收集只在真实入口安装；浏览器预览没有宿主时跳过。
startClientLog();
// 只有真能滚的内部盒子接住滚轮，其余交给外层（见 lib/wheelContainment.ts）。
installWheelContainment();
// 推荐评测日志只在真实入口批量写入；组件测试不启动（见 lib/signals.ts）。
startSignals();

// 首帧前定好界面语言，不先画一帧中文再换。取不到宿主（浏览器预览）或命令失败时按浏览器语言。
async function start() {
  const host = await api.uiLanguage().catch(() => null);
  await applyLanguage(host?.language ?? resolveLanguage("system", navigator.languages));
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
void start();
