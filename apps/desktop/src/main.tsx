import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
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

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
