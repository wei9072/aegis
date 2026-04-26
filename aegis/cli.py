import click
import os
from aegis.agents.llm_adapter import LLMGateway
from aegis.agents.gemini import GeminiProvider


@click.group()
def cli():
    """Aegis Architecture Linter & Generator"""
    pass


@cli.command()
@click.argument('path', type=click.Path(exists=True))
@click.option('--signals', is_flag=True, default=False, help='Show Ring 0.5 structural signals.')
def check(path, signals):
    """Check Ring 0 architectural rules on PATH."""
    from aegis.enforcement.validator import Ring0Enforcer
    from aegis.analysis.signals import SignalLayer
    from aegis.core.bindings import supported_extensions

    enforcer = Ring0Enforcer()
    signal_layer = SignalLayer()
    has_violations = False

    # Walk every extension the registry knows about (Python +
    # tier-2 languages from V1.4–V1.7). Files with unknown
    # extensions are silently skipped — Aegis has no opinion on
    # them, which is the right negative-space stance.
    exts = tuple(supported_extensions())
    files_to_check: list[str] = []
    if os.path.isfile(path):
        if path.lower().endswith(exts):
            files_to_check.append(path)
    else:
        for root, dirs, files in os.walk(path):
            dirs[:] = [d for d in dirs if not d.startswith('.')]
            for f in files:
                if f.lower().endswith(exts):
                    files_to_check.append(os.path.join(root, f))

    if not files_to_check:
        click.echo(f"No supported source files found in {path}")
        click.echo(f"Supported extensions: {', '.join(exts)}")
        return

    for f in files_to_check:
        for v in enforcer.check_file(f):
            has_violations = True
            click.echo(v)

    # Cross-file circular-dependency check is currently
    # Python-only — pass only `.py` files into check_project so
    # other languages aren't misanalysed. (Multi-language
    # circular-dep detection lives in V2+ scope per the rust port
    # plan.)
    py_files = [f for f in files_to_check if f.lower().endswith((".py", ".pyi"))]
    root_dir = path if os.path.isdir(path) else os.path.dirname(path)
    if py_files:
        for v in enforcer.check_project(py_files, root=root_dir):
            has_violations = True
            click.echo(v)

    if signals:
        click.echo("\n--- Ring 0.5 Structural Signals ---")
        for f in files_to_check:
            try:
                sigs = signal_layer.extract(f)
                if sigs:
                    click.echo(f"\n{f}:")
                    for sig in sigs:
                        click.echo(f"  {sig.name} = {sig.value:.0f}  ({sig.description})")
            except Exception as e:
                click.echo(f"  Warning: {f}: {e}", err=True)

    if has_violations:
        click.echo("Aegis check failed.")
        raise SystemExit(1)
    else:
        click.echo("Aegis check passed.")


@cli.command()
@click.argument('prompt')
@click.option('--output', '-o', help='Path to save the generated code.')
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
def generate(prompt, output, model):
    """Generate architecture-compliant code using a generative model."""
    try:
        provider = GeminiProvider(model_name=model)
        gateway = LLMGateway(llm_provider=provider)
        click.echo(f"Generating code via LLMGateway (Model: {model})...")
        safe_code = gateway.generate_and_validate(prompt)
        if output:
            with open(output, 'w') as f:
                f.write(safe_code)
            click.echo(f"Safely generated code written to {output}")
        else:
            click.echo("\n--- Generated Safe Code ---\n")
            click.echo(safe_code)
    except Exception as e:
        click.echo(f"Generation Failed: {e}", err=True)
        raise SystemExit(1)


@cli.command()
@click.argument('task')
@click.argument('path', type=click.Path(exists=True))
@click.option('--scope', multiple=True,
              help='Restrict patches to these paths (repeat for multiple). '
                   'Relative to PATH.')
@click.option('--max-iters', default=3, show_default=True,
              help='Max planner-executor iterations.')
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
@click.option('--no-snippets', is_flag=True, default=False,
              help='Do not send file contents to the planner.')
def refactor(task, path, scope, max_iters, model, no_snippets):
    """Run the AI refactor pipeline against PATH to accomplish TASK."""
    from aegis.runtime import pipeline

    provider = GeminiProvider(model_name=model)
    scope_list = list(scope) if scope else None
    click.echo(f"Running Aegis refactor pipeline (model={model}, max_iters={max_iters})...")
    result = pipeline.run(
        task=task,
        root=path,
        provider=provider,
        scope=scope_list,
        max_iters=max_iters,
        include_file_snippets=not no_snippets,
    )

    click.echo(f"\n--- Pipeline result ---")
    click.echo(f"success: {result.success}")
    click.echo(f"iterations: {result.iterations}")
    before = sum(len(v) for v in result.signals_before.values())
    after = sum(len(v) for v in result.signals_after.values())
    click.echo(f"signals: {before} -> {after}")
    if result.final_plan:
        click.echo(f"final plan goal: {result.final_plan.goal}")
        click.echo(f"final plan strategy: {result.final_plan.strategy}")
        click.echo(f"patches applied: {len(result.final_plan.patches)}")
    if result.validation_errors:
        click.echo("\nValidator errors (last iteration):")
        for err in result.validation_errors:
            click.echo(f"  [{err.kind}] patch={err.patch_id} edit={err.edit_index}: {err.message}")
    if result.error:
        click.echo(f"\nerror: {result.error}")
    if not result.success:
        raise SystemExit(1)


@cli.command()
@click.option('--model', default='gemini-2.5-flash', help='The generative model to use.')
def chat(model):
    """Start an interactive chat session with the Aegis-guarded model."""
    try:
        from prompt_toolkit import PromptSession
        from prompt_toolkit.history import InMemoryHistory

        provider = GeminiProvider(model_name=model)
        gateway = LLMGateway(llm_provider=provider)
        click.echo(f"Starting Aegis Interactive Chat (Model: {model})")
        click.echo("Type 'exit' or 'quit' to end the session.\n")
        session = PromptSession(history=InMemoryHistory())

        while True:
            try:
                user_input = session.prompt("Aegis> ")
                if user_input.lower() in ['exit', 'quit']:
                    break
                if not user_input.strip():
                    continue
                click.echo("Generating and validating...")
                response = gateway.generate_and_validate(user_input)
                click.echo(f"\n{response}\n")
            except KeyboardInterrupt:
                continue
            except EOFError:
                break
            except Exception as e:
                click.echo(f"Error: {e}", err=True)

    except ImportError:
        click.echo("Chat requires 'prompt_toolkit': pip install prompt_toolkit", err=True)
        raise SystemExit(1)
    except Exception as e:
        click.echo(f"Failed to start chat: {e}", err=True)
        raise SystemExit(1)


@cli.group('scenario')
def scenario_group():
    """Drive multi-turn refactor scenarios against a real LLM and print
    the per-iteration decision narrative.

    Distinct from `aegis eval`: that one runs deterministic single-turn
    scenarios with fake providers (CI-grade, free). This one runs
    real-LLM multi-turn scenarios from `tests/scenarios/<name>/` and
    costs tokens — its purpose is to make the iteration loop's
    decisions visible to a human reader, not to gate CI.
    """
    pass


@scenario_group.command('list')
def scenario_list():
    """List the multi-turn scenarios available under tests/scenarios/."""
    import importlib
    from pathlib import Path

    root = Path(__file__).parent.parent / "tests" / "scenarios"
    if not root.is_dir():
        click.echo("No scenarios directory found.", err=True)
        raise SystemExit(1)

    names = sorted(
        d.name for d in root.iterdir()
        if d.is_dir() and (d / "scenario.py").exists()
    )
    if not names:
        click.echo("(no scenarios installed)")
        return

    click.echo("Available scenarios:")
    for n in names:
        try:
            mod = importlib.import_module(f"tests.scenarios.{n}.scenario")
            desc = getattr(mod, "SCENARIO", None)
            full = " ".join((desc.description if desc else "").split())
            summary = full[:90] + ("…" if len(full) > 90 else "")
        except Exception:
            summary = "(could not load scenario module)"
        click.echo(f"  {n:<22}  {summary}")


@scenario_group.command('run')
@click.argument('name')
@click.option('--provider', 'provider_name',
              type=click.Choice(['gemini', 'openrouter', 'groq'], case_sensitive=False),
              default='gemini', show_default=True,
              help='Which provider to drive the pipeline with. '
                   'gemini → google-genai (needs GEMINI_API_KEY or GOOGLE_API_KEY). '
                   'openrouter → OpenAI-compatible gateway over many backends '
                   '(needs OPENROUTER_API_KEY). '
                   'groq → Groq hardware-accelerated inference, free tier with '
                   'generous daily budgets across Llama / Qwen / gpt-oss / Allam '
                   '(needs GROQ_API_KEY).')
@click.option('--model', default=None,
              help='Model id. Defaults: gemini→gemma-4-31b-it, '
                   'openrouter→inclusionai/ling-2.6-1t:free, '
                   'groq→llama-3.3-70b-versatile.')
@click.option('--no-save', is_flag=True, default=False,
              help='Do not write a JSON snapshot to tests/scenarios/<name>/runs/.')
def scenario_run(name, provider_name, model, no_save):
    """Run NAME (e.g. lod_refactor) end-to-end and print the trajectory.

    Prints a labelled block per iteration — Plan / Strategy /
    Validation / Apply / Signals / Decision — so the reasoning loop
    is readable at a glance. JSON snapshot is written under
    tests/scenarios/<name>/runs/ for run-to-run comparison unless
    --no-save is passed.

    Use --provider openrouter to drive the same scenario against any
    OpenRouter-hosted model — this is the path for V1 charter L5
    (cross-model validation): same scenario, different model family.
    """
    import importlib
    import time
    from pathlib import Path
    from tests.scenarios._runner import dump_run, print_trajectory, run_scenario

    try:
        module = importlib.import_module(f"tests.scenarios.{name}.scenario")
    except ImportError as e:
        click.echo(f"scenario {name!r} not found: {e}", err=True)
        raise SystemExit(2)

    if not hasattr(module, "SCENARIO"):
        click.echo(f"tests/scenarios/{name}/scenario.py must export SCENARIO", err=True)
        raise SystemExit(2)

    provider, resolved_model = _build_provider(provider_name.lower(), model)
    label = f"{provider_name.lower()}/{resolved_model}"
    result = run_scenario(module.SCENARIO, provider, model_label=label)
    print_trajectory(result)

    if not no_save:
        runs_dir = Path(__file__).parent.parent / "tests" / "scenarios" / name / "runs"
        timestamp = time.strftime("%Y%m%dT%H%M%S")
        # Slug the model so slashes in OpenRouter ids don't escape the
        # runs/ directory (e.g. inclusionai/ling-2.6-1t:free).
        safe_model = resolved_model.replace("/", "_").replace(":", "_")
        target = runs_dir / f"{timestamp}__{provider_name.lower()}__{safe_model}.json"
        dump_run(result, target)
        click.echo(f"Snapshot:  {target}")

    if not result.pipeline_success:
        raise SystemExit(1)


def _build_provider(provider_name: str, model: str | None):
    """Resolve (provider_name, optional model override) into an
    LLMProvider instance plus the model id actually used."""
    if provider_name == 'gemini':
        chosen = model or 'gemma-4-31b-it'
        return GeminiProvider(model_name=chosen), chosen
    if provider_name == 'openrouter':
        from aegis.agents.openrouter import DEFAULT_MODEL, OpenRouterProvider
        chosen = model or DEFAULT_MODEL
        return OpenRouterProvider(model_name=chosen), chosen
    if provider_name == 'groq':
        from aegis.agents.groq import DEFAULT_MODEL, GroqProvider
        chosen = model or DEFAULT_MODEL
        return GroqProvider(model_name=chosen), chosen
    raise click.UsageError(f"unknown provider {provider_name!r}")


@cli.command('eval')
@click.option('--verbose', '-v', is_flag=True, default=False,
              help='Show extra detail (raised exceptions, event counts) per scenario.')
def run_eval(verbose):
    """Run the Aegis eval harness over the built-in scenarios.

    Each scenario asserts an ordered subsequence of DecisionTrace events.
    Exit code is 0 only when every scenario passes; CI should rely on
    this signal to catch regressions in gate behaviour as new layers
    (ToolCallValidator, policy, intent) are added.
    """
    from aegis.eval import format_results, run_all
    from aegis.eval.scenarios import SCENARIOS

    results = run_all(SCENARIOS)
    click.echo(format_results(results, verbose=verbose))

    if any(not r.passed for r in results):
        raise SystemExit(1)


if __name__ == '__main__':
    cli()
