module.exports = function(context, options) {
  return {
    name: "enable-webassembly-plugin",
    configureWebpack(config, isServer, utils) {
      return {
        experiments: {
          asyncWebAssembly: true,
        },
      };
    }
  };
};
