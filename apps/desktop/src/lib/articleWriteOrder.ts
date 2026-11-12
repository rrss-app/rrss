/**
 * 同一篇文章的状态写入按发起顺序提交。命令可能落到不同阻塞线程，不串行时后发的写入可能先完成，
 * 最终状态与用户最后一次操作相反：打开确认会让宿主顺带标已读、M 键再写未读；连续点赞再点踩也是如此。
 *
 * - 打开之间仍并发，迟到结果由阅读请求票据隔离；只等此前尚未完成的显式写入。
 * - 显式写入等这篇此前的打开和写入都结束（不论成败）后再提交。
 * - 没有需要等待的请求时同步发出，不改变无竞争路径的时序。
 * 不同字段（读状态、评分）可用各自的实例；同一字段的所有写入方必须共用一个实例。
 */
export type ArticleWriteOrder = {
  open: <T>(articleId: number, request: () => Promise<T>) => Promise<T>;
  write: <T>(articleId: number, request: () => Promise<T>) => Promise<T>;
};

type Lane = { opens: Promise<void>; writes: Promise<void>; opening: number; writing: number };

const settled = (promise: Promise<unknown>) => promise.then(() => undefined, () => undefined);

export function createArticleWriteOrder(): ArticleWriteOrder {
  const lanes = new Map<number, Lane>();
  const enter = (id: number) => {
    let lane = lanes.get(id);
    if (!lane) lanes.set(id, lane = { opens: Promise.resolve(), writes: Promise.resolve(), opening: 0, writing: 0 });
    return lane;
  };
  const leave = (id: number, lane: Lane) => {
    if (lane.opening === 0 && lane.writing === 0 && lanes.get(id) === lane) lanes.delete(id);
  };
  return {
    open(id, request) {
      const lane = enter(id);
      const run = lane.writing ? lane.writes.then(request) : request();
      const done = settled(run);
      lane.opening += 1;
      lane.opens = Promise.all([lane.opens, done]).then(() => undefined);
      void done.then(() => { lane.opening -= 1; leave(id, lane); });
      return run;
    },
    write(id, request) {
      const lane = enter(id);
      const run = lane.opening || lane.writing ? Promise.all([lane.opens, lane.writes]).then(request) : request();
      const done = settled(run);
      lane.writing += 1;
      // run 已等过此前的写入，它结束即代表这篇此前的写入都已结束。
      lane.writes = done;
      void done.then(() => { lane.writing -= 1; leave(id, lane); });
      return run;
    },
  };
}
