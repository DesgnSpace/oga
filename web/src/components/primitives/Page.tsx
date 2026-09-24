// Page layout pieces; docs/design.md holds the rules behind their classes.

import type { ReactNode } from "react";

export function PageHeader({
  title,
  description,
  actions,
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="page-header-text">
        <h2 className="page-title">{title}</h2>
        {description ? <p className="page-description">{description}</p> : null}
      </div>
      {actions ? <div className="page-actions">{actions}</div> : null}
    </header>
  );
}

export function Section({
  title,
  description,
  actions,
  className,
  children,
}: {
  title?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <section className={className ? `section ${className}` : "section"}>
      {title || description || actions ? (
        <div className="section-header">
          <div className="section-header-text">
            {title ? <h3 className="section-title">{title}</h3> : null}
            {description ? <p className="section-description">{description}</p> : null}
          </div>
          {actions ? <div className="section-actions">{actions}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return <div className={className ? `card ${className}` : "card"}>{children}</div>;
}

/** As a `label`, a click anywhere on the row reaches its control. */
export function CardRow({
  as: Element = "div",
  title,
  description,
  leading,
  className,
  children,
}: {
  as?: "div" | "label";
  title: ReactNode;
  description?: ReactNode;
  leading?: ReactNode;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <Element className={className ? `card-row ${className}` : "card-row"}>
      {leading}
      <span className="card-row-text">
        <span className="card-row-title">{title}</span>
        {description ? <span className="card-row-description">{description}</span> : null}
      </span>
      {children ? <span className="card-row-control">{children}</span> : null}
    </Element>
  );
}
