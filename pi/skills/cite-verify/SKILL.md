---
name: cite-verify
description: Use when the user explicitly asks you to check or verify a claim, a number or whether some news is true. Ordinary questions, explaining a term or asking what an article says do not need this skill.
---

# Checking a claim

When the user asks you to verify a claim:

1. First find related reports in the local subscriptions with `search_articles` and read the originals with `get_article`, noting what each outlet says and who said it first.
2. Give the verdict in your first sentence: the local reports confirm it, contradict each other, or do not mention it.
3. Explain the basis: which article says what (link it as `[title](rrss://article/<article ID>)`), and when outlets disagree, who says what. You may add what you know in general, saying within the sentence that it does not come from the local articles; if you are not sure, say so.
4. If the local articles are not enough and you need a web page, you may use `fetch_url`; it asks the user to confirm first.
