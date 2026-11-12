use rrss_app::queries;
use rrss_app::views::*;

db_task!(rules_page(filter: String) -> RulesPage => |conn| queries::rules_page(conn, &filter));

db_task!(create_rule(spec: NewRuleSpec) -> i64 =>
    |conn| rrss_app::actions::create_rule(conn, &spec));

db_task!(update_rule(id: i64, spec: NewRuleSpec) -> () =>
    |conn| rrss_app::actions::update_rule(conn, id, &spec));

db_task!(set_rule_action(id: i64, action: i32) -> () =>
    |conn| rrss_app::actions::set_rule_action(conn, id, action));

db_task!(confirm_suggested_rule(id: i64) -> () =>
    |conn| rrss_app::actions::confirm_suggested_rule(conn, id));

db_task!(discard_rule(id: i64) -> () => |conn| rrss_app::actions::discard_rule(conn, id));

db_task!(apply_preset(id: String) -> () =>
    |conn| rrss_app::actions::apply_preset(conn, &id).map(|_| ()));

db_task!(remove_preset(id: String) -> () =>
    |conn| rrss_app::actions::remove_preset(conn, &id).map(|_| ()));

db_task!(confirm_feature(article_id: i64, feed_id: i64, dislike: bool, feed_scope: bool, dimension: String, pattern: String) -> i64 =>
    |conn| rrss_app::actions::confirm_feature(conn, article_id, feed_id, dislike, feed_scope, &dimension, &pattern));

db_task!(rescore() -> u32 => rrss_app::actions::rescore);

db_task!(training_features(article_id: i64, excerpt: Option<String>) -> Vec<FeatureView> =>
    |conn| queries::training_features(conn, article_id, excerpt.as_deref()));

db_task!(hidden_page(scope: Option<String>, list_scope: Option<String>) -> HiddenPage =>
    |conn| queries::hidden_page(conn, scope.as_deref(), list_scope.as_deref()));

db_task!(release_article(id: i64) -> () => |conn| rrss_app::actions::release_article(conn, id));

db_task!(unrelease_article(id: i64) -> () => |conn| rrss_app::actions::unrelease_article(conn, id));

db_task!(allow_rule_for_feed(rule_id: i64, feed_id: i64) -> i64 =>
    |conn| rrss_app::actions::allow_rule_for_feed(conn, rule_id, feed_id));

db_task!(rule_stats() -> RuleStatsPage => queries::rule_stats);

command_handler![
    rules_page,
    rule_stats,
    hidden_page,
    release_article,
    unrelease_article,
    allow_rule_for_feed,
    training_features,
    create_rule,
    update_rule,
    set_rule_action,
    confirm_suggested_rule,
    discard_rule,
    apply_preset,
    remove_preset,
    confirm_feature,
    rescore,
];
