import type { CSSProperties } from "react";
import { Icon } from "./Icon";
import { BrandMark } from "./BrandMark";
import { useTranslation } from "../i18n";

export function AboutOverlay({ onClose, style }: { onClose: () => void; style?: CSSProperties }) {
  const { t } = useTranslation("shell");
  return (
    <div className="overlay center" style={style}>
      <button type="button" className="overlay-dismiss" aria-label={t("about.close")} onClick={onClose} />
      <div className="panel about-panel" role="dialog" aria-modal="true" aria-labelledby="about-title">
        <div className="about-brand">
          <BrandMark />
          <div>
            <h2 id="about-title">RRSS</h2>
            <p className="muted">{t("about.tagline")}</p>
          </div>
        </div>
        <p>{t("about.intro")}</p>
        <ul className="about-facts">
          <li><Icon name="bookmark" size={14} />{t("about.factLocal")}</li>
          <li><Icon name="cpu" size={14} />{t("about.factModels")}</li>
          <li><Icon name="keyboard" size={14} />{t("about.factKeys")}</li>
        </ul>
        <p className="muted about-license">{t("about.license")}</p>
        <div className="row about-actions">
          <button type="button" className="btn" onClick={onClose}>
            {t("about.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
