export function ArticleCardSkeleton({ headline = false }: { headline?: boolean }) {
  return <div className="article-skeleton" data-headline={headline || undefined} aria-hidden="true">
    <div className="article-skeleton-title"><i /><i /></div>
    <i className="article-skeleton-summary" />
    <div className="article-skeleton-meta"><i className="article-skeleton-avatar" /><i /><i /></div>
  </div>;
}
