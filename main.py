import os
import traceback
from dotenv import load_dotenv
from stupidArtnet import StupidArtnet
import time
import signal
import requests
import socketio
import logging
import math
from threading import Thread
from datetime import datetime

load_dotenv()

logging.basicConfig(
    format='[%(asctime)s] %(levelname)-8s %(message)s',
    level=os.environ['LOG_LEVEL'],
    datefmt='%Y-%m-%d %H:%M:%S')

# Global settings
target_ip = '169.254.0.2'		# typically in 2.x or 10.x range
universe = 0 										# see docs
packet_size = 512								# it is not necessary to send whole universe
# End global settings


# Stop thread when necessary
running = True

sio = socketio.Client(logger=True)
a = StupidArtnet(target_ip, universe, packet_size, 40, True, True)

# Logging variables to track the incoming packets
last_packet = datetime(2020, 7, 1)
packets_since = 0
auth_cookie = ''

def get_headers():
    global auth_cookie
    return {'cookie': 'connect.sid=' + auth_cookie}

def parse_array(arr, desired_length):
    int_arr = [max(min(int(x), 255), 0) for x in arr]

    current_length = len(int_arr)

    if current_length >= desired_length:
        # No need to pad, return the original array
        return int_arr
    else:
        # Calculate the number of zeros to pad
        num_zeros = desired_length - current_length

        # Create a new array with zeros and concatenate it with the original array
        padded_array = int_arr + [0] * num_zeros

        return padded_array


def main():
    global sio, running, a, auth_cookie

    url = os.environ['URL'] + '/api/auth/key'
    result = requests.post(url, {'key': os.environ['API_KEY']})

    if result.status_code != 200:
        json = result.json()
        raise Exception("Could not authenticate with core: [HTTP {}]: {}".format(
            result.status_code,
            json['details'] if json['details'] else json['message']),
        )

    auth_cookie = result.cookies.get('connect.sid')

    # Initialize SocketIO
    sio.connect(os.environ['URL'], headers=get_headers, namespaces=['/', '/lights'])

    logging.info('Connected')

    try:
        while running:
            time.sleep(0.5)
    except KeyboardInterrupt:
        running = False
        a.stop()
        a.blackout()
        sio.disconnect()

@sio.event(namespace='/lights')
def dmx_packet(packet):
    global packets_since, last_packet
    parsed_packet = parse_array(packet, packet_size)[0:packet_size]
    packets_since += 1
    a.set(parsed_packet)
    now = datetime.now()
    diff = now - last_packet
    if diff.total_seconds() > 1:
        first_fixture = parsed_packet[:16]
        p = packets_since
        logging.debug(f"Received {p:02} DMX packets since last log (last packet snippet: {first_fixture})")
        packets_since = 0
        last_packet = now

@sio.event
def disconnect():
    a.stop()
    a.blackout()

@sio.event
def connect():
    a.blackout()
    a.start()

if __name__ == '__main__':
    while running:
        try:
            main()
        except Exception as e:
            logging.error(traceback.format_exc())
            logging.info('Something went wrong. Retrying in 5 seconds...')
            time.sleep(5)
