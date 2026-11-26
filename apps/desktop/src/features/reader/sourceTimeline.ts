import { useInfiniteQuery, type InfiniteData, type QueryClient } from "@tanstack/react-query";
import { api, type ArticleView, type FeedsCursor, type FeedsPage } from "../../lib/api";

type SourcePages = InfiniteData<FeedsPage, FeedsCursor | null>;

const sourceKey = (feedId: number, scope: string) => ["reader-source", feedId, scope] as const;

/**
 * 篇目抽屉来源视图的数据：该订阅源的时间线，沿用 `feeds_page` 游标分页。
 * 筛选只属于来源视图，不碰列表视图与焦点的筛选。
 */
export function useSourceTimeline(feedId: number, scope: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: sourceKey(feedId, scope),
    queryFn: async ({ pageParam }): Promise<FeedsPage> => {
      const target = `feed:${feedId}`;
      const page = await api.feedsPage(target, scope, pageParam);
      return page.scope === target ? page : { ...page, articles: [], has_more: false, next_cursor: null };
    },
    initialPageParam: null as FeedsCursor | null,
    getNextPageParam: (last) => last.has_more ? last.next_cursor : undefined,
    enabled: enabled && feedId > 0,
    staleTime: 30_000,
    // 在同源文章里读得久一些，回来时已取的页还在，选中行与滚动位置才接得上。
    gcTime: 30 * 60_000,
    retry: false,
  });
}

export const sourceArticles = (data: { pages: FeedsPage[] } | undefined): ArticleView[] => data?.pages.flatMap((page) => page.articles) ?? [];

/** 从来源视图打开即算读过：先改缓存里的行，不等重新取页。 */
export function markSourceRead(client: QueryClient, feedId: number, id: number) {
  client.setQueriesData<SourcePages>({ queryKey: ["reader-source", feedId] }, (data) => data && {
    ...data,
    pages: data.pages.map((page) => ({ ...page, articles: page.articles.map((row) => row.id === id ? { ...row, read: true } : row) })),
  });
}
