# Narrowcasting Lights Proxy

This script is a simple proxy to forward DMX packets from the Narrowcasting-backend to an ArtNet controller.
Because the ArtNet controller cannot be connected to a general network, a computer will act as a proxy.
This script will connect to the SocketIO server and listens for DMX packets. Once it receives such a
packet, it makes sure it has the right amount of channels and forwards it to the ArtNet controller.

## Installation
1. Install Python 3.11.
2. `pip install -r requirements.txt`.
3. Make sure you have the correct settings configured in `main.py`.
4. `python main.py`.
