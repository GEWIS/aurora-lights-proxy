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

status_thread: Thread | None = None
status_thread_running = False
start_time = 0
latency_ms = 0

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

def send_status_updates():
    global status_thread_running, sio, latency_ms

    while status_thread_running:
        uptime_seconds = int(time.time() - start_time)
        system_timestamp = math.floor(time.time_ns() / 1000000)
        last_send_time = system_timestamp

        def status_update_callback():
            global latency_ms
            callback_time = time.time_ns() / 1000000
            rtt = callback_time - last_send_time
            latency_ms = int(rtt / 2)
            logging.info(f"Latency: {latency_ms} ms")

        sio.emit('status:update', {
            'uptimeSeconds': uptime_seconds,
            'systemTimestamp': system_timestamp,
            'latencyMilliseconds': latency_ms,
        }, callback=status_update_callback)

        time.sleep(5)

def create_status_loop():
    global status_thread, status_thread_running
    status_thread_running = True
    status_thread = Thread(target=send_status_updates)
    status_thread.daemon = True
    status_thread.start()

def stop_status_loop():
    global status_thread, status_thread_running
    if status_thread is None:
        return
    status_thread_running = False
    status_thread.join(timeout=1.0)
    status_thread = None

def main():
    global sio, running, start_time, a, auth_cookie

    start_time = time.time()

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
        stop_status_loop()
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
    stop_status_loop()

@sio.event
def connect():
    a.blackout()
    a.start()
    create_status_loop()

if __name__ == '__main__':
    while running:
        try:
            main()
        except Exception as e:
            logging.error(traceback.format_exc())
            logging.info('Something went wrong. Retrying in 5 seconds...')
            time.sleep(5)
