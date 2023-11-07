import traceback

from stupidArtnet import StupidArtnet
import time
import signal
import requests
import socketio
import logging


# Global settings
target_ip = '169.254.0.2'		# typically in 2.x or 10.x range
universe = 0 										# see docs
packet_size = 512								# it is not necessary to send whole universe
# End global settings


# Stop thread when necessary
running = True
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
    global running
    url = 'http://localhost:3000/auth/mock'
    result = requests.post(url)
    cookie = result.cookies.get('connect.sid')
    a = StupidArtnet(target_ip, universe, packet_size, 40, True, True)
    print(a)

    sio = socketio.Client()
    sio.connect('http://127.0.0.1:3000', headers={'cookie_development': 'connect.sid=' + cookie},
                namespaces=['/lights'])

    a.blackout()
    a.start()

    print('Start listening...')

    @sio.event(namespace='/lights')
    def dmx_packet(packet):
        a.set(parse_array(packet + packet + packet + packet + packet, packet_size)[0:packet_size])

    while running:
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
