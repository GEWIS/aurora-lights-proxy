import os
import traceback
from dotenv import load_dotenv
from stupidArtnet import StupidArtnet
import time
import signal
import requests
import socketio
import logging

load_dotenv()

# Global settings
target_ip = '169.254.0.2'		# typically in 2.x or 10.x range
universe = 0 										# see docs
packet_size = 512								# it is not necessary to send whole universe
# End global settings


# Stop thread when necessary
running = True
# SocketIO exception
crashed = False
def stop_execution(signum, frame):
    global running
    running = False


signal.signal(signal.SIGINT, stop_execution)


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


def main_thread():
    global running, crashed

    crashed = False

    url = os.environ['URL'] + '/api/auth/key'
    result = requests.post(url, {'key': os.environ['API_KEY']})

    if result.status_code != 200:
        json = result.json()
        raise Exception("Could not authenticate with core: [HTTP {}]: {}".format(
            result.status_code,
            json['details'] if json['details'] else json['message']),
        )

    cookie = result.cookies.get('connect.sid')
    a = StupidArtnet(target_ip, universe, packet_size, 40, True, True)
    print(a)

    sio = socketio.Client()
    sio.connect('http://127.0.0.1:3000', headers={'cookie': 'connect.sid=' + cookie},
                namespaces=['/', '/lights'])

    a.blackout()
    a.start()

    print('Start listening...')

    @sio.event(namespace='/lights')
    @sio.event(namespace='/')
    def dmx_packet(packet):
        a.set(parse_array(packet + packet + packet + packet + packet, packet_size)[0:packet_size])

    @sio.event(namespace='/')
    @sio.event(namespace='/lights')
    def disconnect():
        global crashed
        crashed = True
        raise Exception('Socket disconnect')

    while running and not crashed:
        time.sleep(1)

    a.stop()
    a.blackout()
    sio.disconnect()


while running:
    try:
        print('Connecting...')
        main_thread()
    except Exception as e:
        logging.error(traceback.format_exc())
        print('Something went wrong. Try again...')
        time.sleep(5)
