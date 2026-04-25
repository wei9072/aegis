"""Unit tests for the shared edit engine (pure, no I/O)."""
from aegis.ir.patch import Edit, PatchStatus
from aegis.shared.edit_engine import apply_edit, apply_edits, is_ok


def test_applied_when_anchor_unique():
    content = "header\noriginal\nfooter\n"
    edit = Edit(
        old_string="original",
        new_string="renamed",
        context_before="header\n",
        context_after="\nfooter",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.APPLIED
    assert result.matches == 1
    assert new == "header\nrenamed\nfooter\n"


def test_prefix_overlap_is_disambiguated_by_anchor():
    """Regression: str.count finds 'x = 1' inside 'x = 10'. Anchor must prevent this."""
    content = "x = 10\n"  # already refactored
    edit = Edit(
        old_string="x = 1",
        new_string="x = 10",
        context_before="",
        context_after="\n",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.ALREADY_APPLIED
    assert new == content


def test_ambiguous_when_anchor_appears_twice():
    content = "a\nfoo\nb\na\nfoo\nb\n"
    edit = Edit(
        old_string="foo",
        new_string="bar",
        context_before="a\n",
        context_after="\nb",
    )
    _, result = apply_edit(content, edit)
    assert result.status == PatchStatus.AMBIGUOUS
    assert result.matches == 2


def test_already_applied_when_anchored_new_string_present():
    content = "pre\nrenamed\npost\n"
    edit = Edit(
        old_string="original",
        new_string="renamed",
        context_before="pre\n",
        context_after="\npost",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.ALREADY_APPLIED
    assert new == content


def test_not_found_when_neither_anchor_matches():
    content = "unrelated\n"
    edit = Edit(
        old_string="missing",
        new_string="replacement",
        context_before="ctx\n",
        context_after="\nctx",
    )
    _, result = apply_edit(content, edit)
    assert result.status == PatchStatus.NOT_FOUND


def test_empty_context_falls_back_to_raw_uniqueness():
    content = "header\ntoken\nfooter\n"
    edit = Edit(
        old_string="token",
        new_string="newtoken",
        context_before="",
        context_after="",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.APPLIED
    assert new == "header\nnewtoken\nfooter\n"


def test_empty_context_does_not_flag_already_applied_by_mistake():
    """Without context, raw new_string presence must NOT claim ALREADY_APPLIED."""
    content = "renamed\n"
    edit = Edit(
        old_string="original",
        new_string="renamed",
        context_before="",
        context_after="",
    )
    _, result = apply_edit(content, edit)
    assert result.status == PatchStatus.NOT_FOUND


def test_sequential_edits_see_prior_changes():
    content = "a = 1\nb = 2\n"
    edits = [
        Edit(
            old_string="a = 1",
            new_string="a = 10",
            context_before="",
            context_after="\nb",
        ),
        Edit(
            old_string="b = 2",
            new_string="b = 20",
            context_before="a = 10\n",
            context_after="\n",
        ),
    ]
    final, results = apply_edits(content, edits)
    assert all(r.status == PatchStatus.APPLIED for r in results), results
    assert final == "a = 10\nb = 20\n"


def test_is_ok_helper():
    assert is_ok(PatchStatus.APPLIED)
    assert is_ok(PatchStatus.ALREADY_APPLIED)
    assert not is_ok(PatchStatus.NOT_FOUND)
    assert not is_ok(PatchStatus.AMBIGUOUS)


def test_line_level_anchor_without_explicit_newlines_matches():
    """Regression for the syntax_fix scenario: gemma-4-31b-it emitted
    `context_after="    return a + b"` (no leading newline) for a
    `def add(a, b)` line, but the file contains a newline between the
    two lines. Pure byte-concat would NOT_FOUND a perfectly correct
    plan; the matcher's newline-aware fallback must rescue it."""
    content = "def add(a, b)\n    return a + b\n\n\ndef multiply(a, b):\n    return a * b\n"
    edit = Edit(
        old_string="def add(a, b)",
        new_string="def add(a, b):",
        context_before="",
        context_after="    return a + b",  # bare line, no \n
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.APPLIED
    assert result.matches == 1
    assert "def add(a, b):\n    return a + b" in new
    # The other function must be untouched.
    assert "def multiply(a, b):\n    return a * b" in new


def test_line_level_anchor_already_applied_via_newline_form():
    """If the line-level anchor's NEW form (post-fix) is what's on
    disk, ALREADY_APPLIED still fires through the same fallback."""
    content = "def add(a, b):\n    return a + b\n"
    edit = Edit(
        old_string="def add(a, b)",
        new_string="def add(a, b):",
        context_before="",
        context_after="    return a + b",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.ALREADY_APPLIED
    assert new == content


def test_inline_anchor_still_uses_raw_join():
    """An anchor with an explicit trailing `\\n` must keep its inline
    semantics — the newline-aware fallback must NOT double-add a
    newline and break previously-working inline edits."""
    content = "x = 1\n"
    edit = Edit(
        old_string="x = 1",
        new_string="x = 10",
        context_before="",
        context_after="\n",
    )
    new, result = apply_edit(content, edit)
    assert result.status == PatchStatus.APPLIED
    assert new == "x = 10\n"
