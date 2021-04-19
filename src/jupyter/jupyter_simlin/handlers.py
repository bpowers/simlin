import json
from urllib.parse import unquote
from base64 import b64decode, b64encode
from os import fsync

from jupyter_server.base.handlers import APIHandler
import tornado

LOG = None


class RouteHandler(APIHandler):
    # The following decorator should be present on all verb methods (head, get, post,
    # patch, put, delete, options) to ensure only authorized user can request the
    # Jupyter server
    @tornado.web.authenticated
    def get(self, **kwargs):
        model_name = unquote(kwargs['model_name'])
        try:
            with open(model_name, 'rb') as f:
                self.finish(json.dumps({
                    "contents": b64encode(f.read()).decode('utf-8'),
                }))
        except OSError:
            self.send_error(404)

    @tornado.web.authenticated
    def post(self, **kwargs):
        model_name = unquote(kwargs['model_name'])
        body = json.loads(self.request.body)
        contents = b64decode(body['contents'])
        try:
            with open(model_name, 'wb') as f:
                f.write(contents)
                f.flush()
                fsync(f)
        except OSError:
            self.send_error(404)
            return
        if 'xmile' in body and len(body['xmile']) > 0:
            try:
                with open(model_name + '.xmile', 'w') as f:
                    f.write(body['xmile'])
                    f.flush()
                    fsync(f)
            except OSError:
                self.send_error(404)
                return

        self.finish()


def setup_handlers(web_app, log):
    global LOG
    LOG = log
    host_pattern = ".*$"

    base_url = web_app.settings["base_url"]
    route_pattern = r"%sjupyter-simlin/model/(?P<model_name>[^\/]+)" % base_url
    log.info('pattern!!!')
    log.info(route_pattern)
    handlers = [(route_pattern, RouteHandler)]
    web_app.add_handlers(host_pattern, handlers)
