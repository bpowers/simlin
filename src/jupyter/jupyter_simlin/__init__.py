
import json
import os.path as osp

from ._version import __version__
from .handlers import setup_handlers

HERE = osp.abspath(osp.dirname(__file__))

with open(osp.join(HERE, 'labextension', 'package.json')) as file:
    data = json.load(file)


def _jupyter_labextension_paths():
    return [{
        'src': 'labextension',
        'dest': data['name']
    }]


def _jupyter_server_extension_paths():
    return [{
        "module": "jupyter_simlin"
    }]


def load_jupyter_server_extension(server_app):
    """Registers the API handler to receive HTTP requests from the frontend extension.

    Parameters
    ----------
    server_app: jupyterlab.labapp.LabApp
        JupyterLab application instance
    """
    setup_handlers(server_app.web_app)
    server_app.log.info("Registered HelloWorld extension at URL path /juptyer-simlin")


class ProjectWidget(object):
    def __init__(self, file_path: str):
        self.file_path = file_path
        with open(file_path, 'rb') as f:
            self.contents = f.read()

    def _repr_mimebundle_(self, **kwargs):
        data['application/vnd.simlin.widget-view+json'] = {
            'version_major': 1,
            'version_minor': 0,
            'project_id': self.file_path,
            'project_source': self.contents,
        }
        return data


def open_file(file_path: str):
    return ProjectWidget(file_path)
