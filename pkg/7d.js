var api = require('./lib/7d-api.js')
var wrapStorage = require('./lib/wrap-storage.js')
var setWasmModule = require('./lib/set-wasm-module.js')

module.exports = function (opts) {
  if (!opts.storage) throw new Error('opts.storage not provided')
  setWasmModule(api, opts)
  return api.open_mix_f32_f32_f32_f32_f32_f32_f32(wrapStorage(opts.storage))  
}
