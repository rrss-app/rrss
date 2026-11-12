use rrss_app::queries;
use rrss_app::views::*;

db_task!(memory_page(query: String) -> MemoryPage => |conn| queries::memory_page(conn, &query));

db_task!(confirm_memory(id: i64) -> () => |conn| rrss_app::actions::confirm_memory(conn, id));

db_task!(discard_memory(id: i64) -> () => |conn| rrss_app::actions::discard_memory(conn, id));

db_task!(pin_memory(id: i64, pinned: bool) -> () => |conn| rrss_app::actions::pin_memory(conn, id, pinned));

db_task!(clear_memories() -> () => |conn| rrss_app::actions::clear_memories(conn).map(|_| ()));

db_task!(export_memories() -> String => rrss_app::actions::export_memories);

db_task!(edit_memory(id: i64, content: String) -> () =>
    |conn| rrss_app::actions::edit_memory(conn, id, &content));

db_task!(memory_as_interest(id: i64) -> i64 =>
    |conn| rrss_app::actions::memory_as_interest(conn, id));

command_handler![
    memory_page,
    confirm_memory,
    discard_memory,
    pin_memory,
    clear_memories,
    export_memories,
    edit_memory,
    memory_as_interest,
];
