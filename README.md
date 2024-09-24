# Aurora Lights Proxy

This script is a simple proxy to forward DMX packets from the [Aurora core](https://github.com/gewis/narrowcasting-core)
to an ArtNet controller. Because the ArtNet controller cannot be connected to a general network, a computer will act as
a proxy. This script will connect to the SocketIO server and listens for DMX packets. Once it receives such a packet, it
makes sure it has the right amount of channels and forwards it to the ArtNet controller.

## Prerequisites
- Python 3.11.
- An Artnet controller, for example the Showtec NET-2/3 Pocket. The IP should be set to `169.254.0.2`. If you change
this, make sure you also change it in `main.py`.

## Installation
- Create a virtual environment `python -m venv venv`.
- Activate the virtual environment `./venv/Scripts/activate.bat` or `./venv/Scripts/activate`.
- Install requirements `pip install -r requirements.txt`.
- Copy `.env.example` to `.env` and set the URL and API Key.
- Start the script `python main.py`.

When you start the script, make sure the Artnet controller is connected to the host machine. Otherwise, you might get
a lot of socket connection errors. Note that these are probably from the connection to the Artnet controller and not
from the connection with the core. In `main.py`, some more global settings can be found. These settings are tailored
to the situation of GEWIS (with a Showtec NET-2/3 Pocket), but feel free to change these variables to your own needs.
