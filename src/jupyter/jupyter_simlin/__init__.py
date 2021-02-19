
import json
import os.path

from ._version import __version__
from .handlers import setup_handlers

HERE = os.path.abspath(os.path.dirname(__file__))

with open(os.path.join(HERE, 'labextension', 'package.json')) as file:
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
    setup_handlers(server_app.web_app, server_app.log)
    server_app.log.info("Registered HelloWorld extension at URL path /juptyer-simlin")


class ProjectWidget(object):
    def __init__(self, file_path: str, editable=False):
        self.is_editable = editable
        with open(file_path, 'rb') as f:
            self.contents = f.read()
        # if we were passed a Vensim or Stella model, add our suffix onto it
        if not file_path.endswith('.simlin'):
            file_path += '.simlin'
        self.file_path = os.path.abspath(file_path)

    def _repr_mimebundle_(self, **kwargs):
        data['application/vnd.simlin.widget-view+json'] = {
            'version_major': 1,
            'version_minor': 0,
            'project_id': self.file_path,
            'project_initial_source': self.contents,
            'project_is_editable': self.is_editable,
        }
        return data


def open_file(file_path: str, editable=False):
    return ProjectWidget(file_path, editable)
