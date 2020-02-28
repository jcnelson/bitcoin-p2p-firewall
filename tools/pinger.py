#!/usr/bin/env python2

from protocoin.clients import *
from protocoin.serializers import *
from protocoin.fields import *
import socket
import sys
import logging

DEBUG = True

def get_logger(name=None):
    """
    Get virtualchain's logger
    """

    level = logging.CRITICAL
    if DEBUG:
        logging.disable(logging.NOTSET)
        level = logging.DEBUG

    if name is None:
        name = "<unknown>"

    log = logging.getLogger(name=name)
    log.setLevel( level )
    console = logging.StreamHandler()
    console.setLevel( level )
    log_format = ('[%(asctime)s] [%(levelname)s] [%(module)s:%(lineno)d] (' + str(os.getpid()) + '.%(thread)d) %(message)s' if DEBUG else '%(message)s')
    formatter = logging.Formatter( log_format )
    console.setFormatter(formatter)
    log.propagate = False

    if len(log.handlers) > 0:
        for i in xrange(0, len(log.handlers)):
            log.handlers.pop(0)
    
    log.addHandler(console)
    return log

log = get_logger("virtualchain")

PROTOCOL_VERSION = 60002

class Pinger(BitcoinBasicClient):
    coin = None
    timeout = 30

    def __init__(self, coin, host, p2p_port=None):
        """
        Before calling this, the headers must be synchronized
        @last_block_height is *inclusive*
        """
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(self.timeout)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, True)
        try:
            sock.connect((host, p2p_port))
        except socket.error, e:
            log.error("Failed to connect to {}:{}".format(host, p2p_port))
            raise

        super(Pinger, self).__init__(sock)
        self.finished = False;
        
        self.coin = coin
        if p2p_port is None:
            if self.coin == "bitcoin":
                p2p_port = 8333
            elif self.coin == "bitcoin_testnet3" or self.coin == "bitcoin_testnet":
                p2p_port = 18333
            else:
                raise Exception("Invalid coin type '{}'".format(self.coin))

    
    def loop_exit(self):
        """
        Stop the loop
        """
        self.finished = True
        self.close_stream()


    def begin(self):
        """
        This method will implement the handshake of the
        Bitcoin protocol. It will send the Version message,
        and block until it receives a VerAck.
        Once we receive the version, we'll send the verack,
        and begin downloading.
        """
        version = Version()
        version.services = 0    # can't send blocks
        log.debug("send Version")
        self.send_message(version)
        log.debug("sent!")


    def handle_version(self, message_header, message):
        """
        This method will handle the Version message and
        will send a VerAck message when it receives the
        Version message.

        :param message_header: The Version message header
        :param message: The Version message
        """
        log.debug("handle version")
        verack = VerAck()
        log.debug("send VerAck")
        self.send_message(verack)
        self.verack = True
        log.debug("sent!")

        ping = Ping()
        log.debug("send Ping")
        self.send_message(ping)
        log.debug("sent!")


    def handle_ping(self, message_header, message):
        """
        This method will handle the Ping message and then
        will answer every Ping message with a Pong message
        using the nonce received.

        :param message_header: The header of the Ping message
        :param message: The Ping message
        """
        log.debug("handle ping")
        pong = Pong()
        pong.nonce = message.nonce
        log.debug("send Pong")
        self.send_message(pong)
        log.debug("sent!")

        self.begin()


    def run(self):
        self.begin()
        try:
            self.loop()
        except socket.error, se:
            if not self.finished:
                # unexpected
                log.exception(se)
                return False


if __name__ == "__main__":
    pinger = Pinger(sys.argv[1], sys.argv[2], int(sys.argv[3]));
    pinger.run()
