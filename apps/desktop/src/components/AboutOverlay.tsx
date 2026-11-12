import type { CSSProperties } from "react";
import { Icon } from "./Icon";
import { BrandMark } from "./BrandMark";

export function AboutOverlay({ onClose, style }: { onClose: () => void; style?: CSSProperties }) {
  return (
    <div className="overlay center" style={style}>
      <button type="button" className="overlay-dismiss" aria-label="关闭" onClick={onClose} />
      <div className="panel about-panel" role="dialog" aria-modal="true" aria-labelledby="about-title">
        <div className="about-brand">
          <BrandMark />
          <div>
            <h2 id="about-title">RRSS</h2>
            <p className="muted">本地优先的阅读与简报工具</p>
          </div>
        </div>
        <p>装在你电脑上的阅读和简报工具。订阅、文章和对话都留在本机，模型按你的配置去问。</p>
        <ul className="about-facts">
          <li><Icon name="bookmark" size={14} />文章、摘要和整理结果都保存在本机</li>
          <li><Icon name="cpu" size={14} />模型服务由你选择，摘要和简报按你的设置调用</li>
          <li><Icon name="keyboard" size={14} />Ctrl+K 搜索，J / K 选文，Enter 阅读</li>
        </ul>
        <p className="muted about-license">MIT 许可 · Windows / Linux</p>
        <div className="row about-actions">
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
