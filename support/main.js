// jshint devel:true

let drawing = undefined;
let sim = undefined;

// when the browser resizes (or switches between vertical and
// horizontal on mobile), we potentially want to scale our diagram up
// or down.
const scaleDrawing = () => {
  if (!drawing) return;

  let viewport = document.getElementById('viewport');
  if (!viewport) return;
  let bbox = viewport.getBBox();
  let canvas = document.getElementById('model1').getBoundingClientRect();

  // truncate to 2 decimal places
  let scale = (((canvas.width / bbox.width) * 100) | 0) / 100;
  if (scale > 2) scale = 2;
  let wPadding = canvas.width - scale * bbox.width;
  let hPadding = canvas.height - scale * bbox.height;
  drawing.transform(scale, wPadding / 2 - 20, hPadding / 2 - 40);
};

window.addEventListener('resize', scaleDrawing);

const getQueryParams = queryString => {
  queryString = queryString.replace('+', ' ');

  let params = {};
  let tokens = undefined;
  let re = /[?&]?([^=]+)=([^&]*)/g;

  while ((tokens = re.exec(queryString))) {
    params[decodeURIComponent(tokens[1])] = decodeURIComponent(tokens[2]);
  }

  return params;
};

const init = () => {
  let params = getQueryParams(document.location.search);
  let modelPath = 'support/hares_and_foxes.xmile'; // 'population.xmile';
  if ('model' in params) modelPath = params['model'];

  var stocksXYCenter = params['use_stock_xy_as_center'] === 'true';

  sd.load(modelPath, model => {
    drawing = model.project.model('lynxes').drawing('#model1', true, false, stocksXYCenter);

    scaleDrawing();

    sim = model.sim();
    sim.setDesiredSeries(Object.keys(drawing.namedEnts));
    sim.runToEnd().then(data => {
      drawing.visualize(data);
    });
    sim.csv().then(csv => {
      console.log(csv);
    });
  });
};

document.addEventListener('DOMContentLoaded', init);
