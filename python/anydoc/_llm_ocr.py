"""VLM OCR for scanned PDFs, via LiteLLM.

Loaded only when ``to_markdown(..., ocr="llm")`` reaches a PDF whose pages need
OCR, so the optional dependencies (``litellm``, ``pypdfium2``,
``pydantic-settings``) stay out of the base install. Rasterise every page to a
PNG and ask a vision model to transcribe it to Markdown, then stitch the pages
back together.

Configuration comes from ``ANYDOC_OCR_*`` environment variables (see
``_build_settings_class``); the API key is held as a ``SecretStr`` and only
unwrapped at the ``litellm.completion`` call site.
"""

import base64
import io
from concurrent.futures import ThreadPoolExecutor

DEFAULT_PROMPT = (
    "Transcribe this page to clean GitHub-Flavored Markdown. Preserve headings, "
    "lists, tables, and reading order. Do not add commentary or code fences "
    "around the whole answer; output only the Markdown."
)


class LlmOcrConfigError(Exception):
    """A setup problem for ``ocr="llm"``: the extra is not installed, or the
    ``ANYDOC_OCR_*`` settings are missing or invalid. The public wrapper turns
    this into ``LlmOcrError``."""


def _require_deps():
    """Import the optional dependencies, or explain the extra."""
    try:
        import litellm
        import pypdfium2 as pdfium
    except ImportError as error:
        raise LlmOcrConfigError(
            "ocr='llm' needs the 'llm' extra: pip install firecrawl-anydoc[llm]"
        ) from error
    return litellm, pdfium


def _build_settings_class():
    """Construct the settings model lazily: importing pydantic-settings at
    module load would defeat the optional-dependency split."""
    try:
        from pydantic import Field, SecretStr
        from pydantic_settings import BaseSettings, SettingsConfigDict
    except ImportError as error:
        raise LlmOcrConfigError(
            "ocr='llm' needs the 'llm' extra: pip install firecrawl-anydoc[llm]"
        ) from error

    class LlmOcrSettings(BaseSettings):
        """Environment-driven configuration for ``ocr="llm"``.

        Every field is read from ``ANYDOC_OCR_<NAME>``. ``model`` is required;
        it is a LiteLLM model string such as ``openai/gpt-4o-mini`` or
        ``anthropic/claude-sonnet-4-5``. ``api_key`` is optional here because
        LiteLLM also reads the provider's own variable (``OPENAI_API_KEY``,
        ``GEMINI_API_KEY``, ...)."""

        model_config = SettingsConfigDict(env_prefix="ANYDOC_OCR_", extra="ignore")

        model: str
        api_base: "str | None" = None
        api_key: "SecretStr | None" = None
        prompt: str = DEFAULT_PROMPT
        dpi: int = Field(default=200, gt=0)
        max_pages: int = Field(default=100, gt=0)
        page_concurrency: int = Field(default=4, gt=0)
        timeout: float = Field(default=120.0, gt=0)

    return LlmOcrSettings


def _load_settings(model_override):
    from pydantic import ValidationError

    build = _build_settings_class()
    try:
        settings = build()
    except ValidationError as error:
        missing = [
            "ANYDOC_OCR_" + "_".join(str(p) for p in err["loc"]).upper()
            for err in error.errors()
            if err["type"] == "missing"
        ]
        if missing:
            raise LlmOcrConfigError(
                f"ocr='llm' needs {', '.join(missing)} set (a LiteLLM model string, "
                "e.g. ANYDOC_OCR_MODEL=openai/gpt-4o-mini)"
            ) from error
        raise LlmOcrConfigError(f"invalid ANYDOC_OCR_* settings: {error}") from error
    if model_override:
        settings.model = model_override
    return settings


def _rasterise(pdfium, data: bytes, dpi: int, max_pages: int) -> "list[bytes]":
    """Every page of the PDF as PNG bytes."""
    pdf = pdfium.PdfDocument(data)
    try:
        if len(pdf) > max_pages:
            raise LlmOcrConfigError(
                f"PDF has {len(pdf)} pages, over the ocr='llm' limit of {max_pages} "
                "(raise ANYDOC_OCR_MAX_PAGES to allow it)"
            )
        pages = []
        for page in pdf:
            bitmap = page.render(scale=dpi / 72)
            image = bitmap.to_pil()
            buffer = io.BytesIO()
            image.save(buffer, format="PNG")
            pages.append(buffer.getvalue())
            bitmap.close()
            page.close()
        return pages
    finally:
        pdf.close()


def _transcribe_page(litellm, settings, png: bytes) -> str:
    data_uri = "data:image/png;base64," + base64.b64encode(png).decode("ascii")
    kwargs = {"timeout": settings.timeout}
    if settings.api_base:
        kwargs["api_base"] = settings.api_base
    if settings.api_key is not None:
        kwargs["api_key"] = settings.api_key.get_secret_value()
    response = litellm.completion(
        model=settings.model,
        messages=[
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": settings.prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ],
            }
        ],
        **kwargs,
    )
    return response.choices[0].message.content or ""


def parse_llm(data: bytes, model_override: "str | None") -> str:
    """Transcribe a scanned PDF to Markdown with a vision model. Raises
    ``LlmOcrConfigError`` for setup problems and lets ``litellm`` exceptions
    propagate; the public wrapper turns both into ``LlmOcrError``."""
    litellm, pdfium = _require_deps()
    settings = _load_settings(model_override)
    pages = _rasterise(pdfium, data, settings.dpi, settings.max_pages)

    if settings.page_concurrency == 1 or len(pages) == 1:
        transcribed = [_transcribe_page(litellm, settings, png) for png in pages]
    else:
        workers = min(settings.page_concurrency, len(pages))
        with ThreadPoolExecutor(max_workers=workers) as pool:
            transcribed = list(
                pool.map(lambda png: _transcribe_page(litellm, settings, png), pages)
            )

    markdown = "\n\n".join(part.strip() for part in transcribed if part.strip())
    if not markdown:
        raise LlmOcrConfigError("the model returned no Markdown")
    return markdown + "\n"
