use serde_json::{Value, json};

pub fn generic_object_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true
    })
}

pub fn summary_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "one_liner": { "type": "string" },
            "key_points": {
                "type": "array",
                "items": { "type": "string" }
            },
            "facts": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["summary", "one_liner", "key_points", "facts"],
        "additionalProperties": false
    })
}

pub fn segment_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "notes": { "type": "string" },
            "facts": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["notes", "facts"],
        "additionalProperties": false
    })
}

pub fn digest_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": { "type": "string" },
                                    "refs": {
                                        "type": "array",
                                        "items": { "type": "integer", "minimum": 1 },
                                        "minItems": 1,
                                        "maxItems": 3
                                    }
                                },
                                "required": ["text", "refs"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["title", "items"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["sections"],
        "additionalProperties": false
    })
}

pub fn digest_pick_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "picks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "article_id": { "type": "integer" },
                        "reason": { "type": "string" }
                    },
                    "required": ["article_id", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["picks"],
        "additionalProperties": false
    })
}

pub fn cite_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" }
        },
        "required": ["summary"],
        "additionalProperties": false
    })
}

pub fn facts_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "facts": {
                "type": "array",
                "items": { "type": "string" }
            }
        },
        "required": ["facts"],
        "additionalProperties": false
    })
}

pub fn translate_json_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "units": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "i": { "type": "integer" },
                        "t": { "type": "string" }
                    },
                    "required": ["i", "t"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["units"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_schema_requires_only_structured_sections_and_positive_refs() {
        let schema = digest_json_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["sections"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);

        let sections = &schema["properties"]["sections"];
        assert_eq!(sections["type"], "array");
        let section = &sections["items"];
        assert_eq!(section["type"], "object");
        assert_eq!(section["required"], json!(["title", "items"]));
        assert_eq!(section["additionalProperties"], false);
        assert_eq!(section["properties"].as_object().unwrap().len(), 2);
        assert_eq!(section["properties"]["title"]["type"], "string");

        let items = &section["properties"]["items"];
        assert_eq!(items["type"], "array");
        let item = &items["items"];
        assert_eq!(item["required"], json!(["text", "refs"]));
        assert_eq!(item["type"], "object");
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["properties"].as_object().unwrap().len(), 2);
        assert_eq!(item["properties"]["text"]["type"], "string");

        let refs = &item["properties"]["refs"];
        assert_eq!(refs["type"], "array");
        assert_eq!(refs["minItems"], 1);
        assert_eq!(refs["maxItems"], 3);
        assert_eq!(refs["items"]["type"], "integer");
        assert_eq!(refs["items"]["minimum"], 1);
    }
}
