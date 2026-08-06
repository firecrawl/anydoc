// Runs anydoc off the UI thread. Recognition is CPU-bound, so a converter
// built here keeps the page responsive while a document is read.
//
// The module and the two models are fetched once; the converter is kept for
// the lifetime of the worker so the models are parsed once too.
import init, { Converter } from '../../pkg/anydoc_wasm.js'

const MODELS = {
  detection: '/models/text-detection.rten',
  recognition: '/models/text-recognition.rten',
}

const bytes = async (url) => new Uint8Array(await (await fetch(url)).arrayBuffer())

const ready = (async () => {
  await init()
  const [detectionModel, recognitionModel] = await Promise.all([
    bytes(MODELS.detection),
    bytes(MODELS.recognition),
  ])
  return new Converter({ detectionModel, recognitionModel })
})()

self.onmessage = async ({ data: { id, document, format } }) => {
  try {
    const converter = await ready
    self.postMessage({ id, markdown: converter.toMarkdownBytes(document, format) })
  } catch (error) {
    // `code` is what the page branches on: 'ocrInit' means the models are
    // wrong, anything else is the document.
    self.postMessage({ id, error: error.message, code: error.code })
  }
}
