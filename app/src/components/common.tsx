import type { ReactNode } from "react";
import { CATEGORY_LABEL, type CategoryKey } from "../logic";

/** Цветная точка категории; цвет задаётся в CSS классом `cat-<категория>`. */
export function CategoryDot({ category }: { category: CategoryKey }) {
  return <i className={`dot cat-${category}`} title={CATEGORY_LABEL[category]} aria-label={CATEGORY_LABEL[category]} />;
}

export function ReportButton(props: {
  running: boolean;
  hasReport: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  const { running, hasReport, disabled, onClick } = props;
  return (
    <button className="btn" disabled={running || disabled} onClick={onClick}>
      {running ? "Собираю…" : hasReport ? "Пересобрать отчёт" : "Собрать отчёт"}
    </button>
  );
}

export function Banner(props: { title: string; children?: ReactNode; actions?: ReactNode; onClose: () => void }) {
  return (
    <div className="banner" role="alert">
      <div className="banner-body">
        <strong>{props.title}</strong>
        {props.children}
      </div>
      {props.actions}
      <button className="banner-close" aria-label="Закрыть" onClick={props.onClose}>
        ×
      </button>
    </div>
  );
}
