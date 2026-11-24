#!/usr/bin/env python3
"""Exercise failures that would damage a hosted review."""

import copy
import unittest
from unittest.mock import patch

from check_pr_continuity import compare


class Continuity(unittest.TestCase):
    def setUp(self):
        pr = dict(number=91, node_id="review-id", title="Review source bands",
                  state="open", draft=False, created_at="retained", closed_at=None,
                  merged_at=None, author="lilith", assignees=["lilith"],
                  requested_reviewers=[], requested_teams=[],
                  base=dict(ref="main", repo_id=1, sha="base", repository="imazen/example"),
                  head=dict(ref="source-bands", repo_id=1, sha="old", repository="imazen/example"),
                  records={"conversation:1": dict(body_xxh3_128="original")},
                  threads={"thread-id": dict(id="thread-id", isResolved=False, resolvedBy=None)},
                  mergeable=True)
        self.before = dict(schema=2, repository="imazen/example", prs={"91": pr})
        self.after = copy.deepcopy(self.before)

    def test_preserved_review_allows_new_discussion(self):
        self.after["prs"]["91"]["records"]["conversation:2"] = dict(body_xxh3_128="new")
        self.assertEqual(compare(self.before, self.after), [])

    def test_closed_review_is_a_failure(self):
        self.after["prs"].clear()
        self.assertEqual(compare(self.before, self.after), ["PR 91: no longer open"])

    def test_deleted_or_edited_comment_is_a_failure(self):
        self.after["prs"]["91"]["records"]["conversation:1"]["body_xxh3_128"] = "edited"
        self.assertIn("PR 91: existing conversation:1 changed or missing", compare(self.before, self.after))

    def test_changed_stack_base_is_a_failure(self):
        self.after["prs"]["91"]["base"]["ref"] = "different-review"
        self.assertIn("PR 91: base.ref changed", compare(self.before, self.after))

    def test_resolved_thread_is_a_visible_change(self):
        self.after["prs"]["91"]["threads"]["thread-id"]["isResolved"] = True
        self.assertIn("PR 91: existing review thread changed or missing", compare(self.before, self.after))

    @patch("check_pr_continuity.api", return_value=dict(status="diverged"))
    def test_divergent_head_is_a_failure(self, api):
        self.after["prs"]["91"]["head"]["sha"] = "new"
        self.assertIn("PR 91: head update is not a fast-forward", compare(self.before, self.after))

    @patch("check_pr_continuity.api", return_value=dict(status="ahead"))
    def test_forward_head_is_allowed(self, api):
        self.after["prs"]["91"]["head"]["sha"] = "new"
        self.assertEqual(compare(self.before, self.after), [])

    def test_new_conflict_is_a_failure(self):
        self.after["prs"]["91"]["mergeable"] = False
        self.assertIn("PR 91: previously mergeable, now conflicting", compare(self.before, self.after))

    def test_unknown_mergeability_requires_readback(self):
        self.after["prs"]["91"]["mergeable"] = None
        self.assertIn("PR 91: mergeability pending; retry a fresh snapshot", compare(self.before, self.after))

    def test_legacy_discussion_digest_cannot_be_relabelled(self):
        self.before['schema'] = 1
        self.before['prs']['91']['records']['conversation:1'] = dict(body_sha256='legacy')
        self.assertIn('legacy baseline has discussion digests; capture an XXH3 baseline before mutation',
                      compare(self.before, self.after))

    def test_empty_legacy_discussion_keeps_structural_checks(self):
        self.before['schema'] = 1
        self.before['prs']['91']['records'].clear()
        self.assertEqual(compare(self.before, self.after), [])
        self.after['prs']['91']['base']['ref'] = 'wrong'
        self.assertIn('PR 91: base.ref changed', compare(self.before, self.after))


if __name__ == "__main__":
    unittest.main()
